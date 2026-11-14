//! The main window (the approved "hybrid" layout): a top bar with what
//! needs you, the sessions rail, the session in the middle (header, the
//! transcript as turns ([`crate::turns`]), authorization card, composer)
//! and its numbers on the right. Every colour is a Synaps TUI theme role
//! ([`crate::theme`]); every number comes from the daemon ([`crate::stats`]).

use std::collections::{HashMap, HashSet};
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
use crate::fonts::{self, label};
use crate::model::{CONFIRM_NO, CONFIRM_YES, ConnState, Effect, Model, RenameRequest};
use crate::named::{ResumeRequest, RowAction, home_short};
use crate::rail::{Bucket, Chip};
use crate::stats::Stats;
use crate::tabs::{Lru, Place, Saved};
use crate::theme::{self, Metrics, Palette};
use crate::turns::{self, Disp, Kind, Layout, Summaries};

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
}

/// Result lines an open tool row shows.
const RESULT_LINES: usize = 12;
/// Background tabs whose transcript and rows are kept (switching back to
/// one shows it at once, then its attach brings it up to date).
pub(crate) const TAB_CACHE: usize = 8;
/// Tab switches closer together than this attach once, after the last (a
/// held Ctrl+Tab is not an attach, and a full history, per repeat).
const ATTACH_GAP: Duration = Duration::from_millis(120);
/// A splice longer than this reuses the rows of unchanged items.
const REUSE_ROWS: usize = 32;
/// What a tab opened by a create is called until the daemon names it.
pub(crate) const NEW_SESSION: &str = "New session";

/// A background tab's session as it was last on screen: its transcript and
/// numbers, and everything the list built from them (rows, folds, measured
/// heights and scroll position).
pub(crate) struct TabCache {
    transcript: Transcript,
    stats: Stats,
    rows: Vec<RowCache>,
    disp: Vec<Disp>,
    summaries: Summaries,
    list: ListState,
    toggled: HashSet<u64>,
    open_runs: HashSet<u64>,
    synced_busy: bool,
    files_gen: u64,
    waiting_on: Option<u64>,
}

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
/// meant for a row can land on it.
pub(crate) const ARM: Duration = Duration::from_millis(500);
/// The prompt well shows this many lines before it scrolls.
const WELL_LINES: f32 = 12.;
/// A folded side panel's strip.
pub(crate) const STRIP: f32 = 32.;
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
        LastTab
    ]
);

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
    cx.bind_keys(keys);
}

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

/// The text size keys: `=`, `+` (its own key on DE and Nordic layouts)
/// and Shift+`=` (US `+`, as some backends report it).
const fn text_keys(mac: bool) -> [&'static str; 5] {
    if mac { ["cmd-=", "cmd-+", "cmd-shift-=", "cmd--", "cmd-0"] } else { ["ctrl-=", "ctrl-+", "ctrl-shift-=", "ctrl--", "ctrl-0"] }
}

/// The fold keys (`KeyBinding::new` does not resolve `secondary-`, and
/// panics on a keystroke it cannot parse: spelled per OS).
const fn panel_keys(mac: bool) -> (&'static str, &'static str) {
    if mac { ("cmd-b", "cmd-alt-b") } else { ("ctrl-b", "ctrl-alt-b") }
}

/// The rail's and the right panel's widths in the 1280-px layout: the auto
/// widths never go under them, and a dragged width wider than them gives
/// way down to them, not further, when the window narrows.
pub(crate) const FLOORS: (f32, f32) = (276., 296.);
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
        (true, false) => (Side::Open(want_l.min((room - STRIP).max(floor_l)).floor()), Side::Strip),
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

/// Tells a fold key's press from its auto-repeats. Wayland, Windows and
/// macOS repeat a held key without a key-up between; X11 (GPUI does not
/// ask for detectable auto-repeat) sends a key-up right before each repeat,
/// at the same instant. No hand lets go of a key and presses it again in
/// [`FoldKeys::PAIR`].
#[derive(Debug, Default)]
pub(crate) struct FoldKeys {
    pressed: Option<Instant>,
    released: Option<Instant>,
}

impl FoldKeys {
    const PAIR: Duration = Duration::from_millis(20);
    /// A key-up this window never saw (the window lost focus while the key
    /// was down) stops counting after this long without a repeat: longer
    /// than the usual first-repeat delays (X11 660 ms, Windows up to 1 s).
    const STALE: Duration = Duration::from_millis(1100);

    /// A fold key went down at `now`: `true` for a press, `false` for a repeat.
    pub(crate) fn press(&mut self, now: Instant) -> bool {
        let repeat = match (self.pressed, self.released) {
            (None, _) => false,
            (Some(_), Some(up)) => now.saturating_duration_since(up) < Self::PAIR,
            // Not on macOS: AppKit drops the key-up of B when it is let go
            // while Cmd is still held, so a quick unfold (or tapping B with
            // Cmd held) would read as a repeat and do nothing. The cost
            // there: holding Cmd+B flickers the panel.
            (Some(down), None) => !cfg!(target_os = "macos") && now.saturating_duration_since(down) < Self::STALE,
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

/// A fold key the view did not get (the focus is outside it): an app-wide
/// listener, which GPUI runs when nothing on the focus path handled the
/// action. It folds, then gives the view the focus back once the dispatch
/// is over, so the key-up (and the keys after it) reach the view again.
/// A press here is always a press: the key-up of the one before it went
/// where the focus was, not to the view.
fn fold_fallback<A: Action>(rail: bool, view: WeakEntity<AppView>, window: AnyWindowHandle, focus: FocusHandle, cx: &mut App) {
    cx.on_action(move |_: &A, cx| {
        let _ = view.update(cx, |v, cx| {
            v.fold_keys = FoldKeys::default();
            v.fold_key(rail, cx);
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
        let _ = view.update(cx, |v, cx| {
            v.tab_keys = FoldKeys::default();
            v.tab_key(key, cx);
        });
        let focus = focus.clone();
        cx.defer(move |cx| {
            let _ = window.update(cx, |_, window, cx| window.focus(&focus, cx));
        });
    });
}

/// A fold is saved this long after the last one, off the UI thread: a
/// burst of folds is one write.
const SAVE_DELAY: Duration = Duration::from_millis(400);
/// The pointer rests this long on a rail row before its readout opens.
const READOUT_DELAY: Duration = Duration::from_millis(400);

/// `inner` in the centre pane's column ([`Layout::column`], centred).
/// `inner` on the centre column, at the list's own (whole-pixel) padding.
pub(crate) fn column(lay: Layout, inner: impl IntoElement) -> Div {
    h_flex().w_full().pl(px(lay.left)).child(div().flex_none().w(px(lay.column)).min_w_0().child(inner))
}

/// The prompt the authorization card shows.
pub(crate) struct PromptCard {
    /// [`Model::card_key`]: `(epoch, prompt id)`.
    pub(crate) key: (u64, u64),
    shown: Instant,
    /// The body, verbatim (blank lines kept).
    pub(crate) body: SharedString,
    /// What the top bar says about it ([`crate::chrome::prompt_gist`]).
    pub(crate) gist: SharedString,
    scroll: ScrollHandle,
    /// The well was scrolled to its end (or never needed to be).
    read: bool,
    /// Repaints once the arm delay is over.
    _arm: Option<Task<()>>,
}

impl PromptCard {
    /// Keep `card` with the prompt on top of `m` ([`Model::card_key`]): the
    /// same key keeps its arm and read state; a new prompt, the same id in
    /// another attachment or after a reconnect (a restarted daemon numbers
    /// its prompts from 1 again) starts over, unarmed and unread, at `now`.
    /// True when the card is new.
    pub(crate) fn sync(card: &mut Option<PromptCard>, m: &Model, now: Instant) -> bool {
        match (m.card_key(), card.as_mut()) {
            (Some(k), Some(c)) if c.key == k => {
                if c.below().is_some_and(|b| b <= 1.) {
                    c.read = true;
                }
                false
            }
            (Some(k), _) => {
                let pr = &m.transcript.pending_prompts()[0];
                let (body, gist) = (well_body(&pr.prompt), crate::chrome::prompt_gist(&pr.title, &pr.prompt).into());
                *card = Some(PromptCard { key: k, shown: now, body, gist, scroll: ScrollHandle::new(), read: false, _arm: None });
                true
            }
            (None, _) => {
                *card = None;
                false
            }
        }
    }

    pub(crate) fn armed(&self) -> bool {
        self.shown.elapsed() >= ARM
    }

    /// The well's overflow below the view, once it has been laid out.
    fn below(&self) -> Option<f32> {
        if self.scroll.bounds().size.height <= px(0.) {
            return None;
        }
        Some(f32::from(self.scroll.max_offset().y + self.scroll.offset().y).max(0.))
    }
}

/// AUTHORIZE may act: armed, and the whole body was on screen (it fits, or
/// was scrolled to its end). `below`: the well's hidden height, `None`
/// before its first layout.
pub(crate) fn authorize_ready(armed: bool, below: Option<f32>, read: bool) -> bool {
    armed && below.is_some_and(|b| b <= 1. || read)
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
    toggled: HashSet<u64>,
    /// Runs the user unfolded (by their first item's id).
    open_runs: HashSet<u64>,
    /// The transcript's `busy` at the last [`Self::sync_list`]: when it
    /// falls, the last run may fold.
    synced_busy: bool,
    /// The files ledger's generation the rows' counts are from.
    files_gen: u64,
    /// The session the rows are of.
    rows_session: Option<String>,
    /// The last call still unfinished: the one a prompt waits on.
    pub(crate) waiting_on: Option<u64>,
    /// The centre pane's column at the last frame.
    pub(crate) layout: Layout,
    composer: Entity<TextareaState>,
    /// The rail's find (`/`; Enter opens by exact name) and "Name this
    /// session…" (header) inputs.
    pub(crate) find_input: Entity<InputState>,
    name_input: Entity<InputState>,
    /// The accent picker's custom colour and a secret prompt's answer.
    pub(crate) hex_input: Entity<InputState>,
    secret_input: Entity<InputState>,
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
    card: Option<PromptCard>,
    /// The running dot's step, and the 2 Hz timer while a turn runs.
    pub(crate) rail: crate::chrome::RailCache,
    focus: FocusHandle,
    /// Refused input to put back in the composer (needs a `Window`: done
    /// at the next frame).
    restore: Option<String>,
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
    /// The tabs generation saved (or being saved), and the delayed save.
    tabs_saved: u64,
    tabs_save: Option<Task<()>>,
    /// The background tabs' sessions as last seen ([`TAB_CACHE`]).
    pub(crate) tab_cache: Lru<String, TabCache>,
    /// The tab (and its session) the model and the list hold now.
    shown_tab: Option<(u64, Option<String>)>,
    /// A switch's attach waiting out [`ATTACH_GAP`], and when the last
    /// attach went out.
    attach_wait: Option<(String, Option<ResumeRequest>, bool)>,
    attach_due: Option<Task<()>>,
    last_attach: Option<Instant>,
    /// Tells Ctrl+T / Ctrl+W / Ctrl+Shift+T presses from their repeats.
    tab_keys: FoldKeys,
    /// The tab strip's first shown tab when they overflow, and the tab it
    /// last kept in view.
    pub(crate) tab_first: usize,
    pub(crate) tab_follow: Option<u64>,
    /// How many tabs the strip had room for at the last frame.
    pub(crate) tab_room: usize,
    pub(crate) tab_labels: crate::tabstrip::Labels,
    /// The synthetic daemon's attach replies.
    pub(crate) attach_source: Option<AttachSource>,
    _tasks: Vec<Task<()>>,
    _subs: Vec<Subscription>,
}

/// A text run in the display face: caps, tracked, semibold.
pub(crate) fn display(text: impl Into<SharedString>, size: f32, color: Hsla) -> Div {
    div()
        .font_family(fonts::DISPLAY)
        .font_weight(FontWeight::SEMIBOLD)
        .text_size(px(size))
        .text_color(color)
        .whitespace_nowrap()
        .child(text.into())
}

pub(crate) fn mono(text: impl Into<SharedString>, size: f32, color: Hsla) -> Div {
    div().font_family(fonts::MONO).text_size(px(size)).text_color(color).whitespace_nowrap().child(text.into())
}

/// A section header: display caps plus the Japanese sub-label.
pub(crate) fn section(title: &'static str, jp: &'static str, p: &Palette) -> Div {
    h_flex()
        .items_baseline()
        .gap(px(10.))
        .child(display(title, 11.5, p.header))
        .child(div().font_family(fonts::JP).text_size(px(10.)).text_color(p.dim_ch).child(jp))
}

impl AppView {
    pub fn new(
        client: Option<(Client, async_channel::Receiver<Event>)>,
        synthetic: Option<Vec<Event>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 6)
                .submit_on_enter(true)
                .placeholder("Message, or steer a running turn  (Enter sends, Shift+Enter: new line)")
        });
        let find_input = cx.new(|cx| InputState::new(window, cx).placeholder("find · open by name"));
        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("name: a-z, 0-9, '-'"));
        let hex_input = cx.new(|cx| InputState::new(window, cx).placeholder("#RRGGBB"));
        let secret_input = cx.new(|cx| InputState::new(window, cx).masked(true).placeholder("answer (hidden)"));
        let mut subs = vec![
            cx.subscribe_in(&composer, window, |this: &mut Self, input, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { shift: false, .. } = ev {
                    let text = input.read(cx).value().trim().to_string();
                    if !text.is_empty() && this.submit(text) {
                        input.update(cx, |s, cx| s.set_value("", window, cx));
                    }
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
                InputEvent::PressEnter { secondary, .. } => {
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
                if let InputEvent::PressEnter { .. } = ev {
                    let typed = input.read(cx).value().to_string();
                    let req = this.model.rename_request(&typed);
                    this.rename(req);
                    cx.notify();
                }
            }),
            cx.subscribe_in(&hex_input, window, |this: &mut Self, _input, ev: &InputEvent, _window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.apply_custom_accent(cx);
                }
            }),
            cx.subscribe_in(&secret_input, window, |this: &mut Self, _input, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.answer_secret(true, window, cx);
                }
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
        }));
        subs.push(cx.on_app_quit(|this, _| {
            this.flush_panels();
            this.flush_tabs();
            async {}
        }));
        let inputs = Inputs { composer, find_input, name_input, hex_input, secret_input, focus };
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
        Self {
            client,
            model: Model::new(conn),
            rows: Vec::new(),
            disp: Vec::new(),
            summaries: Summaries::default(),
            list,
            toggled: HashSet::new(),
            open_runs: HashSet::new(),
            synced_busy: false,
            files_gen: 0,
            rows_session: None,
            waiting_on: None,
            layout: Layout::of(800., &Metrics::at(theme::DEFAULT_TEXT)),
            composer: i.composer,
            find_input: i.find_input,
            name_input: i.name_input,
            hex_input: i.hex_input,
            secret_input: i.secret_input,
            find: String::new(),
            chip: None,
            expanded: 0,
            rail_list: ListState::new(0, ListAlignment::Top, px(300.)),
            rail_width: 0.,
            hover: None,
            readout: None,
            readout_wait: None,
            panels: theme::Panels::default(),
            panels_dirty: Default::default(),
            panels_writing: Default::default(),
            panels_save: None,
            drag: None,
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
            rail: Default::default(),
            focus: i.focus,
            restore: None,
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
            tabs_save: None,
            tab_cache: Lru::new(TAB_CACHE),
            shown_tab: None,
            attach_wait: None,
            attach_due: None,
            last_attach: None,
            tab_keys: FoldKeys::default(),
            tab_first: 0,
            tab_follow: None,
            tab_room: 0,
            tab_labels: Default::default(),
            attach_source: None,
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
                    this.model.view_dirty = false;
                    for ev in batch {
                        for fx in this.model.apply(ev) {
                            this.effect(fx);
                        }
                    }
                    this.start_tabs();
                    // An attach may have named or merged the visible tab.
                    this.reconcile(None);
                    // An idle daemon's poll changes nothing: no frame, unless
                    // the rail's ages have a new minute to show.
                    if this.model.view_dirty || this.rail.minute() != crate::chrome::now_secs() / 60 {
                        cx.notify();
                    }
                })
                .is_ok()
            })
            .await;
        });
        self._tasks.push(task);
    }

    pub(crate) fn effect(&mut self, fx: Effect) {
        match fx {
            Effect::Restore(text) => {
                self.restore = Some(match self.restore.take() {
                    Some(prev) => format!("{prev}\n{text}"),
                    None => text,
                })
            }
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
                self.last_attach = Some(Instant::now());
                self.model.resuming(req, seq, open);
            }
            Err(e) => self.model.named.error = Some(e.to_string()),
        }
    }

    /// The synthetic daemon (`--offline`, the tab bench): an attach it
    /// answers as a daemon would, on the channel, or never (OPENING).
    fn synthetic_attach(&mut self, session_id: &str, fallback: Option<ResumeRequest>, open: bool) {
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

    /// The rail as the model, find, the chip and the buckets have it now.
    fn refresh_rail(&mut self) {
        let view = crate::rail::View {
            find: std::mem::take(&mut self.find),
            chip: self.chip,
            open: self.panels.history_open,
            expanded: self.expanded,
        };
        self.rail.refresh(&self.model, &view, &self.rail_list);
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
            let shown = self.readout.take().is_some();
            self.readout_wait = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(READOUT_DELAY).await;
                let _ = this.update(cx, |v, cx| {
                    if v.hover == Some(key) {
                        v.readout = Some(key);
                        cx.notify();
                    }
                });
            }));
            if shown {
                cx.notify();
            }
        } else if self.hover == Some(key) {
            self.hover = None;
            self.readout_wait = None;
            if self.readout.take().is_some() {
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
            self.last_attach = Some(Instant::now());
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
        let vis = self.model.tabs.visible().map(|t| (t.id, t.session.clone()));
        if let Some((_, Some(s))) = &vis
            && self.model.wanted_session() == Some(s.as_str())
        {
            // What a cache held of it is older than what attached.
            let _ = self.tab_cache.take(s);
            self.shown_tab = vis;
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
        match vis {
            Some((_, Some(s))) => self.attach_tab(s, hint),
            // A create or a resume: the caller sends it.
            Some((_, None)) => {}
            None => self.detach_all(),
        }
    }

    /// The session on screen goes to the tab cache (if it is one): the
    /// model's transcript and numbers, the rows, folds and the list with
    /// its heights and scroll position. A fresh list takes its place.
    fn stash(&mut self) {
        // The rows first: the transcript may have run ahead of the last frame.
        self.sync_list();
        let key = self.model.transcript.session_id().map(str::to_string);
        let (transcript, stats) = self.model.switch_out();
        self.card = None;
        self.attach_wait = None;
        self.attach_due = None;
        let fresh = ListState::new(0, ListAlignment::Top, px(600.));
        fresh.set_follow_mode(FollowMode::Tail);
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
        };
        self.rows_session = None;
        if let Some(k) = key {
            let tabs = &self.model.tabs;
            self.tab_cache.put(k, cache, |k| tabs.is_open(k));
        }
    }

    /// The visible tab's session as last seen, from the cache, or nothing
    /// until its attach lands.
    fn load(&mut self, session: Option<String>) {
        match session.as_ref().and_then(|s| self.tab_cache.take(s)) {
            Some(c) => {
                self.model.switch_in(c.transcript, c.stats);
                self.rows = c.rows;
                self.disp = c.disp;
                self.summaries = c.summaries;
                self.list = c.list;
                self.toggled = c.toggled;
                self.open_runs = c.open_runs;
                self.synced_busy = c.synced_busy;
                self.files_gen = c.files_gen;
                self.waiting_on = c.waiting_on;
            }
            None => self.model.switch_in(Transcript::new(), Stats::default()),
        }
        self.rows_session = session;
    }

    /// Attach the visible tab's session; within [`ATTACH_GAP`] of the
    /// last attach it waits (shown OPENING), so a burst of switches
    /// attaches once.
    fn attach_tab(&mut self, session: String, hint: Option<(Option<ResumeRequest>, bool)>) {
        let (fallback, open) = hint.unwrap_or_else(|| (self.model.tab_fallback(&session), false));
        self.attach_wait = None;
        if self.client.is_some() && self.last_attach.is_some_and(|t| t.elapsed() < ATTACH_GAP) {
            self.model.showing(&session);
            self.attach_wait = Some((session, fallback, open));
            return;
        }
        self.attach_now(session, fallback, open);
    }

    fn attach_now(&mut self, session: String, fallback: Option<ResumeRequest>, open: bool) {
        if let Some(seq) = self.client.as_ref().and_then(|c| c.attach(session.clone(), AttachMode::Mirror).ok()) {
            self.last_attach = Some(Instant::now());
            self.model.attaching(seq, fallback, open);
            self.model.asked_for(&session);
        } else if self.synthetic_tx.is_some() {
            self.synthetic_attach(&session, fallback, open);
        } else {
            // No daemon: shown, never attached.
            self.model.showing(&session);
        }
    }

    /// The deferred attach, if its tab is still the visible one.
    fn attach_waited(&mut self) {
        self.attach_due = None;
        if let Some((s, fallback, open)) = self.attach_wait.take()
            && self.model.tabs.visible_session() == Some(s.as_str())
            && self.model.active.is_none()
        {
            self.attach_now(s, fallback, open);
        }
    }

    /// The last tab closed: nothing attached (the session keeps running).
    fn detach_all(&mut self) {
        self.attach_wait = None;
        if let Some(c) = &self.client {
            let _ = c.detach();
        }
    }

    /// Restore the saved tabs, then open `--session` (or `--offline`'s
    /// session when nothing was restored), once the first session list
    /// has landed: a saved tab whose session is gone drops, said once.
    fn start_tabs(&mut self) {
        if !self.model.sessions_listed {
            return;
        }
        if let Some(saved) = self.restore_tabs.take()
            && self.model.tabs.is_empty()
        {
            let sessions = &self.model.sessions;
            let (tabs, dropped) = crate::tabs::Tabs::restore(&saved, |id| sessions.iter().any(|s| s.id == id && s.lifecycle != "ending"));
            self.model.tabs = tabs;
            if dropped > 0 {
                self.model.named.notice = Some(match dropped {
                    1 => "1 saved tab closed: its session is no longer open.".into(),
                    n => format!("{n} saved tabs closed: their sessions are no longer open."),
                });
                // Saved again without them.
                self.model.tabs.generation += 1;
            }
            self.tabs_saved = if dropped > 0 { 0 } else { self.model.tabs.generation };
            let sessions = self.model.sessions.clone();
            self.model.tabs.listed(&sessions);
            self.model.rail_gen += 1;
            self.reconcile(None);
        }
        let start = self.start_session.take().or_else(|| self.default_session.take().filter(|_| self.model.tabs.is_empty()));
        self.default_session = None;
        if let Some(id) = start {
            let place = if self.model.tabs.is_empty() { Place::Current } else { Place::New };
            let fallback = self.model.tab_fallback(&id);
            self.open_tab(Open::Attach { session_id: id, fallback, open: false }, place);
        }
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

    /// A tab key. Ctrl+T, Ctrl+W and Ctrl+Shift+T act on a press, never on
    /// its auto-repeats (a held Ctrl+T is not a burst of new sessions).
    pub(crate) fn tab_key(&mut self, key: TabKey, cx: &mut Context<Self>) {
        if matches!(key, TabKey::New | TabKey::Close | TabKey::Reopen) && !self.tab_keys.press(Instant::now()) {
            return;
        }
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
            let job = this.update(cx, |v, _| (theme::settings_path(), v.tabs_key.clone(), v.model.tabs.saved())).ok();
            let Some((Some(path), Some(key), saved)) = job else { return };
            let done = cx.background_spawn(async move { theme::save_tabs(&path, &key, &saved) }).await;
            if let Err(e) = done {
                let _ = this.update(cx, |v, cx| {
                    v.model.last_error = Some(format!("tabs not saved: {e}"));
                    cx.notify();
                });
            }
        }));
    }

    /// A tab save still waiting, written now (the window closes, the app quits).
    fn flush_tabs(&mut self) {
        if self.tabs_save.take().is_some()
            && let (Some(path), Some(key)) = (theme::settings_path(), self.tabs_key.as_deref())
            && let Err(e) = theme::save_tabs(&path, key, &self.model.tabs.saved())
        {
            eprintln!("synaps-desktop: tabs not saved: {e}");
        }
    }

    /// Send the composer's text. `false` keeps it in the composer.
    fn submit(&mut self, text: String) -> bool {
        let Some(c) = &self.client else {
            self.model.last_error = Some("offline".into());
            return false;
        };
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
        let sent = if self.model.transcript.busy() { c.steer(text.clone()) } else { c.submit(text.clone()) };
        if sent.is_err() {
            self.model.last_error = Some("the connection thread has stopped".into());
            return false;
        }
        self.model.sent(text);
        true
    }

    /// A daemon is connected and this attachment may drive it (the
    /// synthetic transport never can).
    fn drives(&self) -> bool {
        self.client.is_some() && self.model.can_send()
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
    fn answer(&mut self, key: (u64, u64), value: Option<String>) -> bool {
        let Some(c) = &self.client else {
            self.model.last_error = Some("offline: there is no daemon to answer".into());
            return false;
        };
        if self.card.as_ref().map(|c| c.key) != Some(key) || self.card_target() != Some(key.1) || !self.model.may_answer(key.1) {
            return false;
        }
        let connection = self.model.active.as_ref().map_or(0, |a| a.connection);
        match c.answer_on(connection, key.1, value) {
            Ok(()) => {
                self.model.answer_sent(key.1);
                true
            }
            Err(_) => {
                self.model.last_error = Some("the connection thread has stopped".into());
                false
            }
        }
    }

    /// AUTHORIZE: only once the whole body has been on screen.
    fn authorize(&mut self, key: (u64, u64)) {
        let ready = self.card.as_ref().is_some_and(|c| c.key == key && authorize_ready(c.armed(), c.below(), c.read));
        if ready {
            self.answer(key, Some(CONFIRM_YES.into()));
        }
    }

    /// A secret prompt's answer (`submit`) or cancel, for the prompt the
    /// card shows; the field is cleared only when it went out.
    fn answer_secret(&mut self, submit: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.card.as_ref().map(|c| c.key) else { return };
        let value = submit.then(|| self.secret_input.read(cx).value().to_string());
        if self.answer(key, value) {
            self.secret_input.update(cx, |s, cx| s.set_value("", window, cx));
        }
        cx.notify();
    }

    /// Keep the card's state with the prompt it shows ([`PromptCard::sync`]);
    /// a new card also clears a half-typed secret meant for the previous one.
    fn sync_card(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if PromptCard::sync(&mut self.card, &self.model, Instant::now())
            && let Some(c) = self.card.as_mut()
        {
            c._arm = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(ARM).await;
                let _ = this.update(cx, |_, cx| cx.notify());
            }));
            self.secret_input.update(cx, |s, cx| s.set_value("", window, cx));
        }
    }

    /// Open the picker with the focus on this view, so Esc reaches the
    /// root's key listener even if the focused input has since gone
    /// (a card answered, a name bar closed): GPUI dispatches a vanished
    /// focus at the window's root, above this view.
    pub(crate) fn open_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.picker_open = true;
        self.picker_note = None;
        self.config_theme = self.config_choice().0.into();
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
        theme::update(cx, |c| c.text = next);
        self.list.remeasure();
        let mut p = self.panels;
        p.text_size = next;
        self.set_panels(p, cx);
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
    /// right panel) as [`toggled`] says, at once (nothing animates); saved
    /// [`SAVE_DELAY`] later. The list re-measures its rows for the new
    /// width by itself.
    pub(crate) fn toggle_panel(&mut self, rail: bool, cx: &mut Context<Self>) {
        // A fold mid-drag commits the drag where it is first; the rest of
        // GPUI's drag then moves nothing (`drag_edge` needs `self.drag`).
        self.end_drag(cx);
        self.drag = None;
        let Some(p) = toggled(self.width, self.panels, rail) else { return };
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
        self.drag = Some(EdgeGrab { rail, x, start: self.panels, width, moved: false });
    }

    /// The pointer is at `x` during a drag of an edge (anywhere: GPUI
    /// reports the moves of its drag outside the handle too).
    fn drag_edge(&mut self, x: f32, cx: &mut Context<Self>) {
        let Some(g) = self.drag.as_mut() else { return };
        g.moved = true;
        let width = if g.rail { g.width + (x - g.x) } else { g.width - (x - g.x) };
        let p = dragged(self.width, g.start, g.rail, width);
        if p != self.panels {
            self.panels = p;
            cx.notify();
        }
    }

    /// The drag let go: what it left is saved, once.
    fn end_drag(&mut self, cx: &mut Context<Self>) {
        if let Some(g) = self.drag.take_if(|g| g.moved) {
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
            self.set_panels(p, cx);
        } else if (if rail { l } else { r }) == Side::Strip {
            self.toggle_panel(rail, cx);
        }
    }

    /// Ctrl+B / Ctrl+Alt+B (Cmd on macOS): a press folds or unfolds, its
    /// key repeats do nothing.
    fn fold_key(&mut self, rail: bool, cx: &mut Context<Self>) {
        if self.fold_keys.press(Instant::now()) {
            self.toggle_panel(rail, cx);
        }
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
        let busy = self.model.transcript.busy();
        let splice = self.model.transcript.take_splice();
        let items = self.model.transcript.items();
        let from = match &splice {
            Some(sp) => {
                if items.len() - sp.start > REUSE_ROWS {
                    // A long tail rebuilt (an attach, a history replace):
                    // rows whose item kept its id and rev keep their strings,
                    // so a cached tab's re-attach rebuilds only what changed.
                    let mut old: HashMap<(u64, u64), RowCache> =
                        self.rows.drain(sp.start.min(self.rows.len())..).map(|r| ((r.id, r.rev), r)).collect();
                    for item in &items[sp.start..] {
                        let r = old.remove(&(item.id, item.rev)).unwrap_or_else(|| row_cache(item, &self.model.stats.files));
                        self.rows.push(r);
                    }
                } else {
                    self.rows.truncate(sp.start);
                    for item in &items[sp.start..] {
                        self.rows.push(row_cache(item, &self.model.stats.files));
                    }
                }
                debug_assert!(self.rows.iter().zip(items).all(|(r, i)| r.id == i.id && r.rev == i.rev));
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
            None => return self.sync_counts(),
        };
        self.waiting_on = items
            .iter()
            .rev()
            .take_while(|i| i.kind != ItemKind::User)
            .find(|i| i.kind == ItemKind::Tool && !turns::finished(i.tool.as_ref().and_then(|t| t.status)))
            .map(|i| i.id);
        let start = turns::turn_start(items, from);
        self.synced_busy = busy;
        self.rebuild_from(start);
        self.sync_counts();
    }

    /// The files ledger committed a call since the rows were built (its
    /// result comes with the history, not with the item's status): the
    /// file tools' counts, and the summaries that sum them, follow it.
    fn sync_counts(&mut self) {
        let ledger = &self.model.stats.files;
        if ledger.generation() == self.files_gen {
            return;
        }
        self.files_gen = ledger.generation();
        let items = self.model.transcript.items();
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
        self.model.transcript = synaps_client::Transcript::new();
        self.rows.clear();
        self.disp.clear();
        self.summaries.clear();
        self.toggled.clear();
        self.open_runs.clear();
        self.list.reset(0);
    }

    /// Display rows from item `start` (a turn's start) on.
    fn rebuild_from(&mut self, start: usize) {
        let items = self.model.transcript.items();
        let new = turns::build(items, start, self.model.transcript.busy(), &self.open_runs);
        self.summaries.update(&new, items, &self.rows);
        let at = self.disp.partition_point(|d| d.item < start);
        if let Some((range, count)) = turns::diff(&self.disp[at..], &new) {
            self.list.splice(at + range.start..at + range.end, count);
            self.disp.splice(at + range.start..at + range.end, new[range.start..range.start + count].iter().copied());
        }
        // A full build per sync: bounded, so long sessions stay usable in dev builds.
        debug_assert!(items.len() > 2_000 || self.disp == turns::build(items, 0, self.model.transcript.busy(), &self.open_runs));
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
        if let Some(ix) = self.disp.iter().position(|d| d.id == id && d.kind == Kind::Work) {
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
        let items = self.model.transcript.items();
        if let Some(start) = items.iter().position(|i| i.id == id).map(|ix| turns::turn_start(items, ix + 1)) {
            self.rebuild_from(start);
        }
    }

    /// The centre column's header: title, model · effort · folder; the
    /// actions (NAME, HALT) top right, the session's state and cost under
    /// them. Fixed sizes (the text step scales the transcript, card and
    /// composer), on the column.
    fn render_header(&self, p: &Palette, cx: &Context<Self>) -> impl IntoElement {
        let lay = self.layout;
        let attached = self.model.active.is_some() && (self.client.is_some() || self.synthetic);
        let title: SharedString = self
            .model
            .transcript
            .title()
            .or(self.model.active_name())
            .or(self.model.tabs.visible().map(|t| t.label.as_str()))
            .unwrap_or("No session")
            .to_string()
            .into();
        let view = self.model.stats.view.as_ref();
        let model = view.map(|v| v.model.as_str()).filter(|m| !m.is_empty()).or(self.model.active_meta().map(|s| s.model.as_str()));
        let effort = view.map(|v| v.effort.clone()).filter(|e| !e.is_empty());
        let cwd = self.model.stats.cwd.as_deref().or(self.model.active_meta().and_then(|s| s.cwd.as_deref())).map(home_short);
        let busy = self.model.transcript.busy() && self.drives();
        let detached = self.model.active.is_none() && self.model.stats.detached;
        // The display face stops at Latin-1 (U+00FF): Polish, Czech or Turkish
        // titles, and other scripts, are set whole in the text face.
        let face = if title.chars().all(fonts::display_covers) { fonts::DISPLAY } else { fonts::SANS };
        // NEEDS YOU / RUNNING / READY beside the cost (glyphs as the rail's).
        let state = attached.then(|| {
            if !self.model.transcript.pending_prompts().is_empty() {
                (crate::rail::State::NeedsYou, label::NEEDS_YOU, p.warn)
            } else if self.model.transcript.busy() {
                (crate::rail::State::Running, label::RUNNING, p.run)
            } else {
                (crate::rail::State::Idle, label::READY, p.ready)
            }
        });
        let actions = h_flex()
            .gap(px(22.))
            .when(attached, |d| {
                d.child(tbtn("name-session", label::NAME, p.dim, p.ink).on_click(cx.listener(|this, _, window, cx| {
                    this.model.toggle_naming();
                    if this.model.named.naming {
                        let current = this.model.active_name().unwrap_or_default().to_string();
                        this.name_input.update(cx, |s, cx| {
                            s.set_value(current, window, cx);
                            s.focus(window, cx);
                        });
                    }
                    cx.notify();
                })))
            })
            .when(busy, |d| {
                d.child(tbtn("halt", label::HALT, p.dim, p.err).on_click(cx.listener(|this, _, _, _| {
                    if let Some(c) = &this.client
                        && this.model.can_send()
                    {
                        let _ = c.abort();
                    }
                })))
            });
        let stat = h_flex()
            .gap(px(14.))
            .items_center()
            .when_some(state, |d, (st, word, color)| {
                let g = match st {
                    // The rail's glyph, in the canvas's colour.
                    crate::rail::State::NeedsYou => div()
                        .relative()
                        .size(px(10.))
                        .flex_none()
                        .child(div().absolute().inset_0().border_1().border_color(p.warn_mark))
                        .child(div().absolute().top(px(3.)).left(px(3.)).size(px(4.)).bg(p.warn_mark)),
                    crate::rail::State::Running => div().size(px(8.)).m(px(1.)).bg(p.run_mark),
                    _ => div().size(px(10.)).border_1().border_color(p.ready),
                };
                d.child(h_flex().gap(px(8.)).items_center().child(g).child(display(word, 13., color)))
            })
            .when_some(self.model.stats.cost.filter(|_| attached), |d, c| d.child(mono(format!("${c:.2}"), 12., p.cost)));
        let mut meta = h_flex().mt(px(6.)).gap(px(10.)).overflow_hidden().whitespace_nowrap();
        let mut sep = false;
        for (text, color) in [
            (detached.then(|| SharedString::from("detached")), p.warn),
            (model.map(|m| crate::named::short_model(m).into()), p.agent),
            (effort.map(SharedString::from), p.dim),
            (cwd.filter(|_| !lay.tight).map(SharedString::from), p.dim),
        ] {
            if let Some(t) = text {
                if sep {
                    meta = meta.child(mono("·", 12., p.dim));
                }
                meta = meta.child(mono(t, 12., color).overflow_hidden().text_ellipsis());
                sep = true;
            }
        }
        let left = v_flex()
            .flex_1()
            .min_w_0()
            .child(
                h_flex()
                    .items_end()
                    .gap(px(16.))
                    .child(display(title, 24., p.ink).font_family(face).line_height(px(28.)).min_w_0().overflow_hidden().text_ellipsis())
                    .when_some(self.model.active_name(), |d, n| d.child(mono(n.to_string(), 12., p.dim).flex_none().pb(px(4.)))),
            )
            .when(sep, |d| d.child(meta));
        div().flex_none().bg(p.canvas).border_b_1().border_color(p.hair).child(column(
            lay,
            h_flex()
                .items_start()
                .gap(px(20.))
                .pt(px(20.))
                .pb(px(16.))
                .child(left)
                .child(v_flex().flex_none().items_end().gap(px(8.)).pt(px(5.)).child(actions).child(stat)),
        ))
    }

    /// The "Name this session…" row under the header.
    fn render_naming(&self, p: &Palette, cx: &Context<Self>) -> impl IntoElement {
        let pending = self.model.named.rename.is_some();
        let mut row = v_flex().py(px(10.)).gap(px(6.)).child(
            h_flex()
                .gap(px(14.))
                .items_center()
                .child(display(label::NAME, 13., p.dim))
                .child(div().flex_1().max_w(px(320.)).child(Input::new(&self.name_input)))
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
                }))),
        );
        if let Some(e) = &self.model.named.rename_error {
            row = row.child(mono(e.clone(), 12., p.err).whitespace_normal());
        }
        // Only for the session it was made for (a switch drops it).
        if let Some(r) = self.model.named.replace.as_ref().filter(|r| self.model.is_active(&r.session_id)) {
            row = row.child(
                h_flex()
                    .gap(px(12.))
                    .child(div().text_size(px(13.)).text_color(p.prose).child(format!("Replace '{}' with '{}'?", r.current, r.asked)))
                    .child(tbtn("name-replace", label::REPLACE, p.warn, p.ink).on_click(cx.listener(|this, _, _, cx| {
                        let req = this.model.replace_request();
                        this.rename(req);
                        cx.notify();
                    }))),
            );
        }
        div().bg(p.canvas).border_b_1().border_color(p.hair).child(column(self.layout, row))
    }

    /// The authorization card: the first pending prompt, answered through
    /// `Client::answer`. The body is shown whole and verbatim in a scrolling
    /// well; AUTHORIZE waits until all of it has been on screen and the
    /// card is armed ([`ARM`]). On the raised prompt surface, outlined and
    /// titled in `warning_color`.
    fn render_prompt(&self, p: &Palette, m: &Metrics, cx: &Context<Self>) -> Option<impl IntoElement> {
        let lay = self.layout;
        let prompts = self.model.transcript.pending_prompts();
        let pr = prompts.first()?;
        let secret = pr.is_secret();
        let card = self.current_card();
        let key = card.map(|c| c.key);
        let sent = self.model.prompt_answered(pr.id);
        let armed = card.is_some_and(|c| c.armed());
        let can = self.drives() && self.model.may_answer(pr.id) && armed;
        let below = card.and_then(|c| c.below());
        let ready = card.is_some_and(|c| authorize_ready(c.armed(), c.below(), c.read));
        let (head, kind) = if secret { (label::SECRET, "secret") } else { (label::AUTHORIZATION, "confirm") };
        let lead: SharedString = pr.title.clone().into();
        let detached = self.model.active.is_none();
        let actions = if detached {
            None
        } else if secret {
            Some(
                h_flex()
                    .gap(px(22.))
                    .items_center()
                    .child(div().w(px((260. * m.r).round())).child(Input::new(&self.secret_input).disabled(!can).text_size(px(m.mono))))
                    .child(
                        fbtn("p-submit", label::SUBMIT, can, p.warn_fill, p, m)
                            .when(can, |d| d.on_click(cx.listener(|this, _, window, cx| this.answer_secret(true, window, cx)))),
                    )
                    .child(
                        tbtn("p-cancel", label::CANCEL, if can { p.err_k } else { p.dim_k }, if can { p.ink } else { p.dim_k })
                            .text_size(px(m.label))
                            .when(can, |d| d.on_click(cx.listener(|this, _, window, cx| this.answer_secret(false, window, cx)))),
                    ),
            )
        } else {
            let yes = can && ready;
            Some(
                h_flex()
                    .gap(px(22.))
                    .items_center()
                    .child(fbtn("p-yes", label::AUTHORIZE, yes, p.warn_fill, p, m).when_some(key.filter(|_| yes), |d, key| {
                        d.on_click(cx.listener(move |this, _, _, cx| {
                            this.authorize(key);
                            cx.notify();
                        }))
                    }))
                    .child(
                        tbtn("p-no", label::DENY, if can { p.err_k } else { p.dim_k }, if can { p.ink } else { p.dim_k })
                            .text_size(px(m.label))
                            .when_some(key.filter(|_| can), |d, key| {
                                d.on_click(cx.listener(move |this, _, _, cx| {
                                    this.answer(key, Some(CONFIRM_NO.into()));
                                    cx.notify();
                                }))
                            }),
                    ),
            )
        };
        let more = below.filter(|&b| b > 1.).map(|b| lines_below(b, m.code_line)).filter(|_| !card.is_some_and(|c| c.read));
        let footer: Option<SharedString> = if detached {
            Some("detached: cannot answer".into())
        } else if sent {
            Some("answer sent, waiting for the daemon".into())
        } else if self.model.switching() {
            Some("switching sessions…".into())
        } else if !self.drives() {
            Some(if self.client.is_none() { "offline: nothing to answer" } else { "read-only here" }.into())
        } else if !secret && let Some(n) = more {
            Some(format!("↓ {n} more lines: scroll to the end to authorize").into())
        } else if prompts.len() > 1 {
            Some(format!("+{} more waiting", prompts.len() - 1).into())
        } else if lay.tight {
            None
        } else {
            Some("the session waits until you answer".into())
        };
        let body = card.map(|c| c.body.clone()).unwrap_or_else(|| pr.prompt.clone().into());
        let r = m.r;
        let well = div()
            .id("prompt-well")
            .mt(px((10. * r).round()))
            .max_h(px(m.code_line * WELL_LINES + 2. * (10. * r).round()))
            .overflow_y_scroll()
            .when_some(card, |d, c| d.track_scroll(&c.scroll))
            .px(px((14. * r).round()))
            .py(px((10. * r).round()))
            .bg(p.canvas)
            .font_family(fonts::MONO)
            .text_size(px(m.mono))
            .line_height(px(m.code_line))
            .text_color(p.well_fg)
            .child(body);
        let g = div()
            .relative()
            .size(px(10.))
            .flex_none()
            .child(div().absolute().inset_0().border_1().border_color(p.warn_mark))
            .child(div().absolute().top(px(3.)).left(px(3.)).size(px(4.)).bg(p.warn_mark));
        Some(
            column(
                lay,
                v_flex()
                    .px(px((20. * r).round()))
                    .py(px((16. * r).round()))
                    .border_1()
                    .border_color(p.warn_mark)
                    .bg(p.card)
                    .child(
                        h_flex()
                            .items_center()
                            .gap(px(12.))
                            .child(g)
                            .child(display(head, m.card_title, p.warn_k).min_w_0().overflow_hidden().text_ellipsis())
                            .child(div().flex_1())
                            .when(!lay.tight, |d| d.child(mono(kind, m.meta, p.dim_k))),
                    )
                    .when(!lead.is_empty(), |d| {
                        d.child(
                            div()
                                .mt(px((8. * r).round()))
                                .text_size(px(m.card_lead))
                                .line_height(px(m.card_lead_line))
                                .text_color(p.prose_k)
                                .child(lead),
                        )
                    })
                    .when(!pr.prompt.is_empty(), |d| d.child(well))
                    .child(h_flex().mt(px((14. * r).round())).items_center().children(actions).child(div().flex_1()).when_some(
                        footer,
                        |d, f| {
                            d.child(
                                mono(f, m.meta, if more.is_some() && !sent { p.warn_k } else { p.dim_k }).overflow_hidden().text_ellipsis(),
                            )
                        },
                    )),
            )
            .pt(px((6. * r).round())),
        )
    }

    /// The composer: on the user surface, the user's bar on its left edge,
    /// SEND (STEER while a turn runs) filled in `prompt_fg`.
    fn render_composer(&self, p: &Palette, m: &Metrics, cx: &Context<Self>) -> impl IntoElement {
        let lay = self.layout;
        let busy = self.model.transcript.busy();
        let can_send = self.drives();
        let r = m.r;
        let hint = if self.client.is_none() && !self.synthetic {
            "offline: no daemon to send to"
        } else if self.client.is_none() {
            "offline: synthetic transport, nothing is sent"
        } else if self.model.switching() {
            "switching sessions…"
        } else if !can_send {
            "not attached"
        } else if busy {
            "steer: applied at the next turn boundary"
        } else {
            "send: starts a turn"
        };
        column(
            lay,
            v_flex()
                .px(px(m.pad_x))
                .pt(px(m.pad_y))
                .pb(px((11. * r).round()))
                .bg(p.user)
                .relative()
                .border_1()
                .border_color(p.edge)
                // The user's bar over the left border.
                .child(div().absolute().left(px(-1.)).top(px(-1.)).bottom(px(-1.)).w(px(2.)).bg(p.you_mark))
                .text_size(px(m.body))
                .line_height(px(m.body_line))
                .text_color(p.ink)
                // The kit sizes its own text: the step is set on it.
                .child(
                    Textarea::new(&self.composer)
                        .appearance(false)
                        .disabled(!can_send)
                        .text_size(px(m.body))
                        .line_height(px(m.body_line))
                        .text_color(p.ink),
                )
                .child(
                    h_flex()
                        .mt(px((10. * r).round()))
                        .items_center()
                        .gap(px(20.))
                        .child(if lay.tight {
                            div().flex_1()
                        } else {
                            mono(hint, m.meta, p.dim_u).flex_1().min_w_0().overflow_hidden().text_ellipsis()
                        })
                        .child(fbtn("send", if busy { label::STEER } else { label::SEND }, can_send, p.btn_fill, p, m).on_click(
                            cx.listener(|this, _, window, cx| {
                                let text = this.composer.read(cx).value().trim().to_string();
                                if !text.is_empty() && this.submit(text) {
                                    this.composer.update(cx, |s, cx| s.set_value("", window, cx));
                                }
                                cx.notify();
                            }),
                        )),
                ),
        )
        .flex_none()
        .pt(px((10. * r).round()))
        .pb(px((22. * r).round()))
    }
}

/// A text button: tracked caps, `color`, `hover` on hover.
pub(crate) fn tbtn(id: &'static str, text: &'static str, color: Hsla, hover: Hsla) -> Stateful<Div> {
    display(text, 13., color).id(id).cursor_pointer().hover(move |d| d.text_color(hover))
}

/// A filled button (AUTHORIZE, SEND): its label is the chrome colour on
/// `fill` (lifted until that reads at 4.5:1), dimmed when it cannot act.
pub(crate) fn fbtn(id: &'static str, text: &'static str, enabled: bool, fill: Hsla, p: &Palette, m: &Metrics) -> Stateful<Div> {
    let (bg, fg) = if enabled { (fill, p.chrome) } else { (p.edge, p.dim) };
    div()
        .id(id)
        .h(px((30. * m.r).round().max(28.)))
        .px(px(16.))
        .flex()
        .flex_none()
        .items_center()
        .bg(bg)
        .when(enabled, |d| d.cursor_pointer().hover(|d| d.opacity(0.88)))
        .child(display(text, m.label, fg))
}

pub(crate) fn row_cache(item: &Item, ledger: &files::Ledger) -> RowCache {
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
    };
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
            r.arg = match &r.preview {
                Some(pv) if !pv.title.is_empty() => pv.title.clone().into(),
                _ => one_line(&t.input, 200).into(),
            };
            let lines: Vec<&str> = t.result.lines().collect();
            r.result_more = lines.len().saturating_sub(RESULT_LINES);
            r.result = lines.iter().take(RESULT_LINES).copied().collect::<Vec<_>>().join("\n").into();
        }
        ItemKind::Thinking => {
            r.text = item.text.clone().into();
            r.arg = one_line(&item.text, 200).into();
        }
        _ => r.text = item.text.clone().into(),
    }
    r
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
    name_input: Entity<InputState>,
    hex_input: Entity<InputState>,
    secret_input: Entity<InputState>,
    focus: FocusHandle,
}

impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::diag::mark("first_render");
        if let Some(text) = self.restore.take() {
            // Refused input goes back where it was typed, ahead of anything
            // typed since.
            self.composer.update(cx, |s, cx| {
                let now = s.value().trim().to_string();
                let text = if now.is_empty() { text } else { format!("{text}\n{now}") };
                s.set_value(text, window, cx);
            });
        }
        if std::mem::take(&mut self.model.named.opened) {
            self.find_input.update(cx, |s, cx| s.set_value("", window, cx));
            self.find.clear();
        }
        self.sync_list();
        self.sync_card(window, cx);
        self.refresh_rail();
        if let Some(b) = self.bench.as_mut() {
            b.on_render(&self.list, window);
        }
        let p = theme::palette(cx);
        let m = theme::metrics(cx);
        // A drag released outside the window has no drop: it ends here.
        if self.drag.is_some_and(|g| g.moved) && !cx.has_active_drag() {
            self.end_drag(cx);
        }
        if let Some((rail, w)) = self.bench.as_ref().and_then(BenchState::drag_width) {
            self.panels = dragged(self.width, self.panels, rail, w);
        }
        // Wide windows widen the side panels; narrow ones fold the open
        // numbers, then the open rail, to strips before the session gives.
        self.width = window.viewport_size().width.into();
        self.height = window.viewport_size().height.into();
        let (rail, right) = sides(self.width, self.panels, crate::chrome::MARK);
        if let Side::Open(w) = rail {
            self.rail_width = w;
        }
        // The centre pane's column. The list spans the pane (the wheel
        // scrolls it anywhere, the labels hang in its padding) with the
        // column as its item width: while the column is capped, a resize or
        // a panel drag changes only the padding, and the list keeps every
        // measured row (it re-measures when its item width changes).
        let pane = (self.width - crate::chrome::MARK - rail.width() - right.width()).max(0.);
        let lay = Layout::of(pane, &m);
        if (lay.hang, lay.tight) != (self.layout.hang, self.layout.tight) {
            self.list.remeasure();
        }
        self.layout = lay;
        let tabs = self.render_tabs(pane, &p, cx);
        self.schedule_tabs_save(cx);
        if self.attach_wait.is_some() && self.attach_due.is_none() {
            let left = ATTACH_GAP.saturating_sub(self.last_attach.map_or(ATTACH_GAP, |t| t.elapsed()));
            self.attach_due = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(left).await;
                let _ = this.update(cx, |v, cx| {
                    v.attach_waited();
                    cx.notify();
                });
            }));
        }
        let (pad_l, pad_r) = lay.padding();
        let list = list(self.list.clone(), cx.processor(|this, ix, _window, cx| this.render_disp(ix, cx)))
            .size_full()
            .pl(px(pad_l))
            .pr(px(pad_r))
            .pt(px((8. * m.r).round()))
            .pb(px((28. * m.r).round()));
        let main = v_flex()
            .flex_1()
            .h_full()
            .min_w_0()
            .bg(p.canvas)
            .child(tabs)
            .child(self.render_header(&p, cx))
            .when(self.model.named.naming && self.model.active.is_some(), |d| d.child(self.render_naming(&p, cx)))
            .child(div().flex_1().min_h_0().child(list))
            .children(self.render_prompt(&p, &m, cx))
            .when_some(self.model.last_error.clone(), |d, e| d.child(column(lay, mono(e, m.meta, p.err).pt(px(6.)).whitespace_normal())))
            .child(self.render_composer(&p, &m, cx));
        h_flex()
            .id("root")
            .track_focus(&self.focus)
            // Esc closes the picker, wherever the focus is; in find it clears
            // it and leaves it.
            .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                if this.picker_open && ev.keystroke.key == "escape" {
                    this.close_picker(window, cx);
                    cx.stop_propagation();
                } else if ev.keystroke.key == "escape" && this.find_input.read(cx).focus_handle(cx).is_focused(window) {
                    this.find_input.update(cx, |s, cx| s.set_value("", window, cx));
                    this.find.clear();
                    window.focus(&this.focus, cx);
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            // `/` (nothing else focused) goes to find.
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                let m = &ev.keystroke.modifiers;
                if ev.keystroke.key == "/" && !(m.control || m.alt || m.platform) && this.focus.is_focused(window) {
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
            .map(|d| tab_actions(d, cx))
            .capture_key_up(cx.listener(|this, ev: &KeyUpEvent, _, _| {
                let k = &ev.keystroke.key;
                if k.eq_ignore_ascii_case("b") {
                    this.fold_keys.release(Instant::now());
                } else if k.eq_ignore_ascii_case("t") || k.eq_ignore_ascii_case("w") {
                    this.tab_keys.release(Instant::now());
                }
            }))
            .relative()
            .size_full()
            .bg(p.chrome)
            .text_color(p.ink)
            .font_family(fonts::SANS)
            .text_size(px(13.))
            .child(
                v_flex().flex_1().h_full().min_w_0().child(self.render_top(&p, rail, cx)).child(
                    h_flex()
                        .flex_1()
                        .min_h_0()
                        .map(|d| match rail {
                            Side::Open(w) => d.child(self.render_rail(w, &p, cx)),
                            Side::Strip => d.child(self.render_rail_strip(&p, cx)),
                        })
                        .child(main)
                        .map(|d| match right {
                            Side::Open(w) => d.child(self.render_right(w, &p, cx)),
                            Side::Strip => d.child(self.render_right_strip(&p, cx)),
                        }),
                ),
            )
            .children(self.render_edge(true, rail, &p, cx))
            .children(self.render_edge(false, right, &p, cx))
            .when(self.picker_open, |d| d.child(deferred(self.render_picker(&p, right, cx)).with_priority(1)))
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that brings in gpui's `test` attribute through `gpui_kit::*`.
    use std::time::{Duration, Instant};

    use super::{
        CENTRE_MIN, FoldKeys, Side, TabKey, authorize_ready, dragged, lines_below, one_line, panel_keys, side_widths, sides, tab_keys,
        text_keys, toggled, well_body,
    };
    use crate::theme::Panels;

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

    /// The tab keys parse on every OS, each keystroke means one thing, and
    /// none is a key the inputs (composer, find, names) or an IME use: the
    /// list is gpui-base 0.9.1's input bindings (`input/base/state.rs`).
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
        for mac in [false, true] {
            let (a, b) = panel_keys(mac);
            let mut app: Vec<_> = [a, b].into_iter().chain(text_keys(mac)).map(|k| (parse(k), None)).collect();
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
            let nth: Vec<usize> = tab_keys(mac).iter().filter_map(|(_, w)| if let TabKey::Nth(n) = w { Some(*n) } else { None }).collect();
            assert_eq!(nth, (0..8).collect::<Vec<_>>());
            for want in [TabKey::New, TabKey::Close, TabKey::Reopen, TabKey::Next, TabKey::Prev, TabKey::Last] {
                assert!(tab_keys(mac).iter().any(|(_, w)| *w == want), "{want:?}");
            }
        }
    }

    /// Wide windows widen the panels; folded ones are strips at any width;
    /// an open panel the window cannot fit is a strip too, never hidden.
    #[test]
    fn side_panels_follow_the_window() {
        let open = Panels::default();
        assert_eq!(sides(1280., open, 30.), (Side::Open(276.), Side::Open(296.)));
        assert_eq!(sides(1920., open, 30.), (Side::Open(276.), Side::Open(296.)));
        assert_eq!(sides(2560., open, 30.), (Side::Open(340.), Side::Open(358.)));
        assert_eq!(sides(1000., open, 30.), (Side::Open(276.), Side::Strip));
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

    /// The PR #7 rule, before panels could be dragged.
    fn pr7_sides(window: f32, panels: Panels) -> (Side, Side) {
        let (rail_w, right_w) = side_widths(window);
        let rail = if !panels.rail_collapsed && window >= 760. { Side::Open(rail_w) } else { Side::Strip };
        let right =
            if !panels.right_collapsed && window - 30. - rail.width() - right_w >= CENTRE_MIN { Side::Open(right_w) } else { Side::Strip };
        (rail, right)
    }

    fn centre(window: f32, (l, r): (Side, Side)) -> f32 {
        window - 30. - l.width() - r.width()
    }

    fn hand(rail: Option<u16>, right: Option<u16>) -> Panels {
        Panels { rail_width: rail, right_width: right, ..Panels::default() }
    }

    /// Auto widths lay out as before at every width; dragged widths stay in
    /// their limits, and never squeeze the centre under its minimum except
    /// where the auto rail already does (a narrow window, rail at 276).
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
                        assert!(c >= CENTRE_MIN || w <= 276., "{window} {p:?} {shown:?}");
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
        assert_eq!(sides(1100., hand(Some(480), Some(520)), 30.), (Side::Open(284.), Side::Open(305.)));
        // ...then the right one is a strip and the rail stops at the centre's minimum...
        assert_eq!(sides(1000., hand(Some(480), Some(520)), 30.), (Side::Open(458.), Side::Strip));
        // ...then the PR #7 narrow layout.
        assert_eq!(sides(800., hand(Some(480), Some(520)), 30.), (Side::Open(276.), Side::Strip));
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
        assert_eq!(right, hand(None, Some(494)));
        assert_eq!(sides(1250., right, m), (Side::Open(276.), Side::Open(494.)));
        assert_eq!(dragged(1920., open, false, 900.).right_width, Some(520));
        // 770 px, rail only: as wide as the narrow layout already shows it, no wider.
        assert_eq!(rail(770., 400.).rail_width, Some(276));
        assert_eq!(rail(770., 240.).rail_width, Some(240));
        // Panels that had given way: the other one is held where it is, so
        // the dragged edge follows the pointer.
        let both = hand(Some(480), Some(520));
        let p = dragged(1070., both, true, 250.);
        assert_eq!(p, hand(Some(250), Some(305)));
        assert_eq!(sides(1070., p, m), (Side::Open(250.), Side::Open(305.)));
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
        assert!(k.press(ms(0)), "a press");
        // Held (no key-up): the repeats after the delay, then every 33 ms.
        assert!(!k.press(ms(500)));
        assert!(!k.press(ms(533)));
        k.release(ms(600));
        assert!(k.press(ms(700)), "let go, pressed again");
        // X11: a key-up and its repeat at the same instant.
        k.release(ms(1200));
        assert!(!k.press(ms(1200)));
        k.release(ms(1233));
        assert!(!k.press(ms(1234)));
        // A real release, then a quick second press (a double tap).
        k.release(ms(1300));
        assert!(k.press(ms(1380)));
        // A key-up this window never saw: a press long after is a press.
        assert!(!k.press(ms(2000)), "within the first-repeat delay");
        assert!(k.press(ms(3200)));
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
        assert!(k.press(ms(0)), "fold");
        assert!(k.press(ms(300)), "unfold, Cmd still held, no key-up seen");
        k.release(ms(400));
        assert!(!k.press(ms(400)), "a key-up and a press at the same instant");
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

    #[test]
    fn one_line_arguments() {
        assert_eq!(one_line("  \nfirst\nsecond", 80), "first …");
        assert_eq!(one_line("abcdef", 3), "abc …");
    }
}
