//! The palette (polish.md P16): one overlay, two modes. Ctrl+P opens GO TO
//! (open sessions, the attached session's workers, history, Settings and
//! Usage); Ctrl+K opens COMMANDS (the app's actions, each with its key,
//! and the engine's slash commands). The other key, or Tab, flips the mode;
//! Esc closes it and gives the focus back. Opened by key, so it is there at
//! once; the selection quad and the list height spring on `snap`. No GPUI
//! in the logic ([`Palette`], [`score`]); [`AppView::render_palette`] draws
//! it on the overlay box.

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::app::{AppView, display, mono};
use crate::fonts::TextPx as _;
use crate::fonts::{self, label, size};
use crate::named::RowAction;
use crate::rail::State;
use crate::subagents::Liveness;
use crate::theme::Palette as Colors;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    GoTo,
    Commands,
}

/// A group head (C3's sub-labels where the font has them).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Group {
    Sessions,
    Workers,
    History,
    Views,
    Commands,
}

impl Group {
    fn head(self) -> (&'static str, Option<&'static str>) {
        match self {
            Group::Sessions => (label::SESSIONS, Some(fonts::JP_SESSIONS)),
            Group::Workers => (label::WORKERS, Some(fonts::JP_WORKERS)),
            Group::History => (label::HISTORY, Some(fonts::JP_HISTORY)),
            Group::Views => (label::VIEWS, None),
            Group::Commands => (label::COMMANDS, Some(fonts::JP_COMMANDS)),
        }
    }
}

/// What the app does when picked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AppAct {
    NewSession,
    ToggleRail,
    ToggleRight,
    Settings,
    Usage,
    ThemeNext,
    ThemePrev,
    Accent,
    Motion,
    FoldAll,
    UnfoldAll,
    JumpWaiting,
    Favourite,
    Halt,
    Detach,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Act {
    /// A session row (attach, or resume from history), as the rail's.
    Row(RowAction),
    /// A worker of the attached session (its tray id): its view.
    Worker(u64),
    App(AppAct),
    /// An engine slash command ([`ENGINE`]).
    Engine(&'static str),
}

/// A row's lead glyph: a session's state, a worker's, or none.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Lead {
    None,
    Session(State),
    Worker(Liveness),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Entry {
    pub group: Group,
    pub act: Act,
    pub title: SharedString,
    /// `~/folder · age · cost`, matched after the title.
    pub meta: SharedString,
    /// The key hint (or `owner` on an engine command).
    pub key: Option<SharedString>,
    pub lead: Lead,
    /// Its order when nothing is typed (state rank, recency), and the tie
    /// break when something is.
    pub rank: u32,
    /// It cannot act now: why (picked, the note says so).
    pub off: Option<SharedString>,
}

impl Entry {
    pub fn new(group: Group, act: Act, title: impl Into<SharedString>) -> Self {
        Entry { group, act, title: title.into(), meta: SharedString::default(), key: None, lead: Lead::None, rank: 0, off: None }
    }
}

/// The slash commands the daemon runs (`EngineCommand`): a static list, the
/// daemon has no listing (polish.md §7). Each is owner-only, as every
/// engine command; `true`: it needs an argument (picked bare, the palette
/// waits for it).
pub(crate) const ENGINE: [(&str, bool, &str); 7] = [
    ("compact", false, "summarise the conversation so far"),
    ("model", true, "switch model: provider/model"),
    ("context", false, "what the context holds"),
    ("trace", false, "the request trace"),
    ("budget", false, "the session's budget"),
    ("workers", false, "the session's workers"),
    ("confirms", false, "allow-all approvals: status, reset"),
];

/// `/name arg` as typed (the composer's text, the palette's query): an
/// engine command and its argument, if `name` is one of [`ENGINE`].
pub(crate) fn engine_of(text: &str) -> Option<(&'static str, &str)> {
    let t = text.trim();
    let t = t.strip_prefix('/').unwrap_or(t);
    let (name, arg) = t.split_once(char::is_whitespace).map_or((t, ""), |(n, a)| (n, a.trim()));
    ENGINE.iter().find(|e| e.0 == name).map(|e| (e.0, arg))
}

/// What the composer's note line says of a slash command's answer: its
/// first line, or why not.
pub(crate) fn reply_note(name: &str, result: &Result<String, String>) -> String {
    let first = |t: &str| t.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").to_string();
    match result {
        Ok(t) if first(t).is_empty() => format!("/{name}: done"),
        Ok(t) => format!("/{name}: {}", first(t)),
        Err(why) => format!("/{name} not run: {}", first(why)),
    }
}

/// Rows shown at most (the rest are a query away): history can list
/// thousands, and every shown row is drawn each frame.
pub(crate) const SHOWN_MAX: usize = 120;

/// How well `q` (lowercase, non-empty) matches `text`: a prefix best, then
/// a word's start, then anywhere, then its letters in order (tighter,
/// earlier runs score higher). `None`: no match.
pub(crate) fn score(q: &str, text: &str) -> Option<u32> {
    let t = text.to_lowercase();
    if t.starts_with(q) {
        return Some(4000 - len_pen(&t));
    }
    if let Some(at) = t.match_indices(q).map(|(i, _)| i).find(|&i| starts_word(&t, i)) {
        return Some(3000 - pen(at) - len_pen(&t));
    }
    if let Some(at) = t.find(q) {
        return Some(2000 - pen(at) - len_pen(&t));
    }
    // Subsequence: each letter after the last, greedily; a letter at a
    // word's start or right after the last one scores.
    let mut s: i32 = 1000;
    let mut chars = t.char_indices();
    let mut last: Option<usize> = None;
    for qc in q.chars() {
        let (i, _) = chars.find(|&(_, c)| c == qc)?;
        s += match last {
            Some(l) if i == l + 1 => 8,
            _ if starts_word(&t, i) => 6,
            _ => -(i as i32 - last.map_or(0, |l| l as i32)).min(20),
        };
        last = Some(i);
    }
    Some(s.clamp(1, 1999) as u32)
}

fn pen(at: usize) -> u32 {
    (at as u32).min(200)
}

fn len_pen(t: &str) -> u32 {
    (t.len() as u32 / 8).min(100)
}

fn starts_word(t: &str, i: usize) -> bool {
    i == 0 || t[..i].chars().next_back().is_some_and(|c| !c.is_alphanumeric())
}

/// One line of the list: a group head, or a shown row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Line {
    Head(Group),
    Row(usize),
}

pub(crate) const ROW_H: f32 = 32.;
pub(crate) const HEAD_H: f32 = 28.;

#[derive(Clone, Debug)]
pub(crate) struct Palette {
    pub mode: Mode,
    /// What is typed, lowercased and trimmed.
    pub query: String,
    /// What is typed, trimmed (an engine command's argument keeps its case).
    pub raw: String,
    /// Each mode's rows (rebuilt when the lists change).
    pub goto: Vec<Entry>,
    pub commands: Vec<Entry>,
    /// The mode's rows the query keeps, best first (indices).
    pub shown: Vec<usize>,
    /// Index into `shown`.
    pub sel: usize,
    /// The query held an engine command and its argument (`model x`).
    pub arg: Option<(&'static str, String)>,
    /// The selection was moved by a key since the rows were filtered: a
    /// rebuild keeps it on its row; else it stays on the first.
    pub moved: bool,
}

impl Palette {
    pub fn new(mode: Mode, goto: Vec<Entry>, commands: Vec<Entry>) -> Self {
        let mut p =
            Palette { mode, query: String::new(), raw: String::new(), goto, commands, shown: Vec::new(), sel: 0, arg: None, moved: false };
        p.filter();
        p
    }

    pub fn entries(&self) -> &[Entry] {
        match self.mode {
            Mode::GoTo => &self.goto,
            Mode::Commands => &self.commands,
        }
    }

    /// Tab, or the other mode's key: the other mode, its first row.
    pub fn flip(&mut self) {
        self.set_mode(if self.mode == Mode::GoTo { Mode::Commands } else { Mode::GoTo });
    }

    pub fn set_mode(&mut self, mode: Mode) {
        if self.mode != mode {
            self.mode = mode;
            self.filter();
        }
    }

    pub fn set_query(&mut self, typed: &str) {
        let raw = typed.trim();
        if raw != self.raw {
            self.raw = raw.to_string();
            self.query = raw.to_lowercase();
            self.filter();
        }
    }

    /// New rows (the lists changed): the selection stays on its row.
    pub fn set_entries(&mut self, goto: Vec<Entry>, commands: Vec<Entry>) {
        if goto == self.goto && commands == self.commands {
            return;
        }
        let was = self.selected().map(|e| e.act.clone()).filter(|_| self.moved);
        (self.goto, self.commands) = (goto, commands);
        self.filter();
        if let Some(i) = was.and_then(|a| self.shown.iter().position(|&i| self.entries()[i].act == a)) {
            self.sel = i;
            self.moved = true;
        }
    }

    /// The rows the query keeps: nothing typed, every row in group order by
    /// rank; else the matches, groups by their best match, rows by score
    /// (title first, then meta), ties by rank.
    fn filter(&mut self) {
        let q = self.query.as_str();
        // Commands: `model provider/x` is /model with that argument.
        self.arg = None;
        if self.mode == Mode::Commands
            && let Some((name, arg)) = engine_of(&self.raw).filter(|(_, a)| !a.is_empty())
        {
            self.arg = Some((name, arg.to_string()));
        }
        let entries = self.entries();
        let mut hits: Vec<(usize, u32)> = entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| {
                if q.is_empty() {
                    return Some((i, 0));
                }
                if let Some((name, _)) = self.arg
                    && e.act == Act::Engine(name)
                {
                    return Some((i, 5000));
                }
                let title = score(q, &e.title);
                let meta = || score(q, &e.meta).filter(|&s| s >= 2000).map(|s| s - 1500);
                title.or_else(meta).map(|s| (i, s))
            })
            .collect();
        let best = |g: Group| hits.iter().filter(|(i, _)| entries[*i].group == g).map(|h| h.1).max().unwrap_or(0);
        let mut groups: Vec<Group> = entries.iter().map(|e| e.group).collect();
        groups.sort();
        groups.dedup();
        if !q.is_empty() {
            groups.sort_by_key(|&g| std::cmp::Reverse(best(g)));
        }
        let place = |g: Group| groups.iter().position(|&x| x == g).unwrap_or(usize::MAX);
        hits.sort_by(|a, b| {
            let (ea, eb) = (&entries[a.0], &entries[b.0]);
            place(ea.group).cmp(&place(eb.group)).then(b.1.cmp(&a.1)).then(ea.rank.cmp(&eb.rank)).then(a.0.cmp(&b.0))
        });
        self.shown = hits.into_iter().map(|h| h.0).take(SHOWN_MAX).collect();
        self.sel = 0;
        self.moved = false;
    }

    pub fn selected(&self) -> Option<&Entry> {
        self.shown.get(self.sel).map(|&i| &self.entries()[i])
    }

    /// ↓ / ↑, wrapping; PageDown / PageUp by `page` rows, stopping at the
    /// ends.
    pub fn step(&mut self, by: isize, wrap: bool) {
        let n = self.shown.len() as isize;
        if n == 0 {
            return;
        }
        let to = self.sel as isize + by;
        self.sel = if wrap { to.rem_euclid(n) } else { to.clamp(0, n - 1) } as usize;
        self.moved = true;
    }

    /// The list's lines: a head before each group's first row.
    pub fn lines(&self) -> Vec<Line> {
        let entries = self.entries();
        let mut out = Vec::with_capacity(self.shown.len() + 5);
        let mut last = None;
        for (k, &i) in self.shown.iter().enumerate() {
            let g = entries[i].group;
            if last != Some(g) {
                out.push(Line::Head(g));
                last = Some(g);
            }
            out.push(Line::Row(k));
        }
        out
    }
}

/// Where a line starts, and the list's whole height.
pub(crate) fn line_y(lines: &[Line]) -> (Vec<f32>, f32) {
    let mut y = 0.;
    let ys = lines
        .iter()
        .map(|l| {
            let at = y;
            y += if matches!(l, Line::Head(_)) { HEAD_H } else { ROW_H };
            at
        })
        .collect();
    (ys, y)
}

/// The query field's placeholder.
pub(crate) const PLACEHOLDER: &str = "go to a session, worker or view · Tab: commands";
const PLACEHOLDER_COMMANDS: &str = "run a command, or /compact, /model provider/name · Tab: go to";

/// The palette's keys while it is open, on the root ahead of its query
/// field's own bindings (capture phase): ↑/↓ and PageUp/PageDown move the
/// selection, Enter picks, Tab flips the mode, Esc closes it. Closed, they
/// go on to whatever has the focus.
pub(crate) fn capture_keys(d: Stateful<Div>, cx: &Context<AppView>) -> Stateful<Div> {
    use gpui_kit::base::input::{Enter, Escape, IndentInline, MoveDown, MovePageDown, MovePageUp, MoveUp, OutdentInline};
    fn on<A: Action>(
        d: Stateful<Div>,
        cx: &Context<AppView>,
        f: impl Fn(&mut AppView, &A, &mut Window, &mut Context<AppView>) + 'static,
    ) -> Stateful<Div> {
        d.capture_action(cx.listener(move |this: &mut AppView, a: &A, window, cx| {
            if this.palette.is_some() {
                cx.stop_propagation();
                f(this, a, window, cx);
            }
        }))
    }
    let d = on(d, cx, |this, _: &MoveUp, _, cx| this.palette_step(-1, true, cx));
    let d = on(d, cx, |this, _: &MoveDown, _, cx| this.palette_step(1, true, cx));
    let d = on(d, cx, |this, _: &MovePageUp, _, cx| this.palette_step(-PAGE, false, cx));
    let d = on(d, cx, |this, _: &MovePageDown, _, cx| this.palette_step(PAGE, false, cx));
    let d = on(d, cx, |this, _: &IndentInline, window, cx| this.palette_flip(window, cx));
    let d = on(d, cx, |this, _: &OutdentInline, window, cx| this.palette_flip(window, cx));
    let d = on(d, cx, |this, _: &Escape, window, cx| this.close_palette(window, cx));
    on(d, cx, |this, _: &Enter, window, cx| {
        // Its repeats go nowhere once it has closed (a held Enter on a
        // session would send the composer's draft): as a menu's pick.
        this.menu_picked = true;
        this.palette_pick(window, cx);
    })
}

/// An app action's row: what it does, its title, key, meta and why it
/// cannot act now.
type AppRow = (AppAct, &'static str, Option<SharedString>, SharedString, Option<&'static str>);

/// PageUp / PageDown's step.
const PAGE: isize = 8;

impl AppView {
    /// Ctrl+P (`GoTo`) or Ctrl+K (`Commands`): the palette opens in that
    /// mode, at once, its query empty and focused; open, it switches to it.
    /// A menu or the theme picker closes first.
    pub(crate) fn open_palette(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = self.palette.as_mut() {
            if p.mode != mode {
                p.set_mode(mode);
                self.palette_placeholder(window, cx);
                self.palette_scroll.scroll_to_item(0);
            }
            cx.notify();
            return;
        }
        self.close_menu();
        self.gear_menu = false;
        if self.picker_open {
            self.close_picker(window, cx);
        }
        self.palette_back = window.focused(cx).filter(|f| *f != self.palette_input.focus_handle(cx));
        let (goto, commands) = self.palette_entries(cx);
        self.palette_gen = self.palette_sig(cx);
        self.palette = Some(Palette::new(mode, goto, commands));
        self.palette_sel.set(None);
        self.palette_scroll.scroll_to_item(0);
        self.palette_input.update(cx, |s, cx| {
            s.set_value("", window, cx);
            s.focus(window, cx);
        });
        self.palette_placeholder(window, cx);
        log::debug!(target: "synaps_desktop::palette", "open {mode:?}");
        cx.notify();
    }

    fn palette_placeholder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = match self.palette.as_ref().map(|p| p.mode) {
            Some(Mode::Commands) => PLACEHOLDER_COMMANDS,
            _ => PLACEHOLDER,
        };
        self.palette_input.update(cx, |s, cx| s.set_placeholder(text, window, cx));
    }

    /// Esc, a click outside, or a pick: it closes, and the focus goes back
    /// where it was (the composer, usually).
    pub(crate) fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.take().is_none() {
            return;
        }
        match self.palette_back.take() {
            Some(h) => window.focus(&h, cx),
            None => window.focus(&self.focus, cx),
        }
        cx.notify();
    }

    fn palette_flip(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = self.palette.as_mut() {
            p.flip();
        }
        self.palette_placeholder(window, cx);
        self.palette_scroll.scroll_to_item(0);
        cx.notify();
    }

    fn palette_step(&mut self, by: isize, wrap: bool, cx: &mut Context<Self>) {
        let Some(p) = self.palette.as_mut() else { return };
        p.step(by, wrap);
        // The quad is the list's first child: a line's is one on.
        if let Some(k) = p.lines().iter().position(|l| *l == Line::Row(p.sel)) {
            // A group's first row brings its head into view with it.
            self.palette_scroll.scroll_to_item(if k > 0 && p.sel == 0 { 1 } else { k + 1 });
        }
        cx.notify();
    }

    /// What the rows are built from: rebuilt (selection kept) when it changes.
    fn palette_sig(&self, cx: &App) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let m = &self.model;
        (m.rail_gen, m.input_gate().is_ok(), m.transcript.pending_prompts().len(), self.halt_shown()).hash(&mut h);
        (m.tabs.len(), m.tabs.active(), m.active.as_ref().map(|a| &a.session_id)).hash(&mut h);
        for r in &self.tray.rows {
            (r.id, r.live as u8, &r.step, r.watchable).hash(&mut h);
        }
        (crate::motion::Level::current(cx) as u8, &cx.global::<crate::theme::Scheme>().choice.theme).hash(&mut h);
        h.finish()
    }

    /// The open palette follows the lists (a session's state, a worker
    /// ending), each frame they changed.
    pub(crate) fn refresh_palette(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.is_none() {
            return;
        }
        let sig = self.palette_sig(cx);
        if sig == self.palette_gen {
            return;
        }
        self.palette_gen = sig;
        let (goto, commands) = self.palette_entries(cx);
        if let Some(p) = self.palette.as_mut() {
            p.set_entries(goto, commands);
        }
    }

    /// Both modes' rows.
    fn palette_entries(&self, cx: &App) -> (Vec<Entry>, Vec<Entry>) {
        let mac = cfg!(target_os = "macos");
        let m = &self.model;
        let now = crate::chrome::now_secs();
        let mut goto = Vec::new();
        // SESSIONS: what the daemon holds, by state (needs you first), each
        // open tab's key as its trail.
        for (i, s) in m.sessions.iter().enumerate().filter(|(_, s)| s.lifecycle != "ending") {
            let st = m.state(s);
            let title = match (s.name.as_deref().filter(|n| !n.is_empty()), s.title.as_deref().filter(|t| !t.is_empty())) {
                (Some(n), _) => n.to_string(),
                (None, Some(t)) => t.to_string(),
                (None, None) if s.messages == 0 => "New session".into(),
                (None, None) => s.id.clone(),
            };
            let since = s.state_since.as_deref().or(Some(s.created_at.as_str()).filter(|c| !c.is_empty()));
            let mut meta: Vec<String> = Vec::new();
            meta.extend(s.cwd.as_deref().filter(|c| !c.is_empty()).map(crate::named::home_short));
            meta.extend(since.map(|t| crate::named::relative(t, now)).filter(|a| !a.is_empty()));
            if s.cost_usd >= 0.005 {
                meta.push(format!("${:.2}", s.cost_usd));
            }
            let tab = m
                .tabs
                .iter()
                .filter(|t| t.kind.attaches())
                .position(|t| t.kind == crate::tabs::Kind::Session && t.session.as_deref() == Some(&s.id));
            let mut e = Entry::new(Group::Sessions, Act::Row(RowAction::Attach(s.id.clone())), title);
            e.meta = meta.join(" · ").into();
            e.lead = Lead::Session(st);
            e.rank = state_rank(st) * 10_000 + i as u32;
            e.key = tab.filter(|&t| t < 8).map(|t| format!("{}{}", if mac { "⌘" } else { "Ctrl+" }, t + 1).into());
            goto.push(e);
        }
        // WORKERS: the attached session's, as its chips.
        for (i, r) in self.tray.rows.iter().enumerate() {
            let mut e = Entry::new(Group::Workers, Act::Worker(r.id), r.name.clone());
            e.meta = if r.step.is_empty() { crate::glyph::worker(r.live).2.into() } else { r.step.clone() };
            e.lead = Lead::Worker(r.live);
            e.rank = i as u32;
            e.off = (!r.watchable).then(|| r.tip.clone());
            goto.push(e);
        }
        // HISTORY: the saved sessions the rail lists, newest first.
        for (i, (_, v, _)) in crate::rail::history(m).into_iter().enumerate() {
            let name = v.name.as_deref().filter(|n| !n.is_empty());
            let title = name.or(v.title.as_deref().filter(|t| !t.is_empty())).unwrap_or(&v.id).to_string();
            let mut meta: Vec<String> = Vec::new();
            meta.extend(v.cwd.as_deref().filter(|c| !c.is_empty()).map(crate::named::home_short));
            meta.extend(
                v.updated_at.as_deref().or(v.created_at.as_deref()).map(|t| crate::named::relative(t, now)).filter(|a| !a.is_empty()),
            );
            let req = crate::named::ResumeRequest {
                label: name.unwrap_or(&title).to_string(),
                target: v.id.clone(),
                cwd: v.cwd.clone().filter(|c| !c.is_empty()),
                by_name: false,
            };
            let mut e = Entry::new(Group::History, Act::Row(RowAction::Resume(req)), title);
            e.meta = meta.join(" · ").into();
            e.lead = Lead::Session(m.saved_state(v));
            e.rank = i as u32;
            goto.push(e);
        }
        let k = |linux: &'static str, macos: &'static str| -> Option<SharedString> { Some(if mac { macos } else { linux }.into()) };
        for (act, title, key) in [(AppAct::Settings, "Settings", k("Ctrl+,", "⌘,")), (AppAct::Usage, "Usage", k("Ctrl+Shift+U", "⌘⇧U"))]
        {
            let mut e = Entry::new(Group::Views, Act::App(act), title);
            e.key = key;
            goto.push(e);
        }
        // COMMANDS: the app's actions, then the engine's.
        let attached = m.active.is_some();
        let level = crate::motion::Level::current(cx);
        let theme = cx.global::<crate::theme::Scheme>().choice.theme.clone();
        let apps: [AppRow; 15] = [
            (AppAct::NewSession, "New session", k("Ctrl+T", "⌘T"), "in a new tab".into(), None),
            (AppAct::ToggleRail, "Toggle sessions rail", k("Ctrl+B", "⌘B"), "".into(), None),
            (AppAct::ToggleRight, "Toggle right panel", k("Ctrl+Alt+B", "⌘⌥B"), "".into(), None),
            (AppAct::Settings, "Settings", k("Ctrl+,", "⌘,"), "".into(), None),
            (AppAct::Usage, "Usage", k("Ctrl+Shift+U", "⌘⇧U"), "".into(), None),
            (AppAct::ThemeNext, "Next theme", None, format!("now {theme}").into(), None),
            (AppAct::ThemePrev, "Previous theme", None, format!("now {theme}").into(), None),
            (AppAct::Accent, "Accent colour…", None, "the theme picker".into(), None),
            (AppAct::Motion, "Motion level", None, format!("now {}, next {}", level.key(), next_level(level).key()).into(), None),
            (AppAct::FoldAll, "Fold all runs", None, "".into(), (!attached).then_some("no session on screen")),
            (AppAct::UnfoldAll, "Unfold all runs", None, "".into(), (!attached).then_some("no session on screen")),
            (
                AppAct::JumpWaiting,
                "Jump to what is waiting",
                k("Ctrl+J", "⌃J"),
                "the authorize card".into(),
                m.transcript.pending_prompts().is_empty().then_some("nothing is waiting on you"),
            ),
            (AppAct::Favourite, "Favourite this session", k("Ctrl+D", "⌘D"), "".into(), (!attached).then_some("no session on screen")),
            (
                AppAct::Halt,
                "HALT the running turn",
                k("Esc Esc", "Esc Esc"),
                "".into(),
                (!self.halt_shown()).then_some("no turn here to halt"),
            ),
            (
                AppAct::Detach,
                "Detach (close tab)",
                k("Ctrl+W", "⌘W"),
                "the session keeps running".into(),
                (!attached).then_some("no session on screen"),
            ),
        ];
        let mut commands = Vec::new();
        for (i, (act, title, key, meta, off)) in apps.into_iter().enumerate() {
            let mut e = Entry::new(Group::Commands, Act::App(act), title);
            e.key = key;
            e.meta = meta;
            e.rank = i as u32;
            e.off = off.map(SharedString::from);
            commands.push(e);
        }
        let gate = m.input_gate().err();
        for (i, (name, _, about)) in ENGINE.iter().enumerate() {
            let mut e = Entry::new(Group::Commands, Act::Engine(name), format!("/{name}"));
            e.meta = (*about).into();
            e.key = Some("owner".into());
            e.rank = 100 + i as u32;
            e.off = gate.map(|why| format!("/{name} is the session owner's: {why}").into());
            commands.push(e);
        }
        (goto, commands)
    }

    /// Enter on (or a click at) the selected row.
    pub(crate) fn palette_pick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.palette.as_ref() else { return };
        let Some(e) = p.selected().cloned() else { return };
        // An engine command that needs its argument: the field asks for it.
        if let Act::Engine(name) = e.act
            && e.off.is_none()
            && ENGINE.iter().any(|x| x.0 == name && x.1)
            && !p.arg.as_ref().is_some_and(|a| a.0 == name)
        {
            let text = format!("/{name} ");
            self.palette_input.update(cx, |s, cx| s.set_value(text.clone(), window, cx));
            if let Some(p) = self.palette.as_mut() {
                p.set_query(&text);
            }
            cx.notify();
            return;
        }
        let arg = p.arg.clone();
        self.close_palette(window, cx);
        if let Some(why) = e.off {
            self.watch_note = Some(why);
            cx.notify();
            return;
        }
        log::debug!(target: "synaps_desktop::palette", "pick {:?}", e.act);
        match e.act {
            Act::Row(action) => {
                for fx in self.model.open_row(&action) {
                    self.effect(fx);
                }
            }
            Act::Worker(id) => {
                if let Some(r) = self.tray.rows.iter().find(|r| r.id == id) {
                    let who = crate::wtabs::Who::new(r.id, r.name.clone(), r.call.clone());
                    self.watch_worker(&who, cx);
                }
            }
            Act::App(a) => self.app_act(a, window, cx),
            Act::Engine(name) => {
                let arg = arg.filter(|a| a.0 == name).map(|a| a.1).unwrap_or_default();
                self.run_command(name, arg);
            }
        }
        cx.notify();
    }

    /// Slash command `name` with `arg`, on the composer's path and the
    /// palette's: the engine runs it (`EngineCommand`, owner-only); where
    /// this window may not drive the session, the note says so politely
    /// and nothing is sent. Its answer comes back as the composer's note.
    pub(crate) fn run_command(&mut self, name: &'static str, arg: String) -> bool {
        if let Err(why) = self.model.input_gate() {
            self.watch_note = Some(format!("/{name} is the session owner's: {why}").into());
            self.model.view_dirty = true;
            return false;
        }
        let Some(c) = &self.client else {
            self.watch_note = Some(format!("offline: /{name} not run").into());
            return false;
        };
        if c.engine_command(name, arg).is_err() {
            self.model.last_error = Some("the connection thread has stopped".into());
            return false;
        }
        log::debug!(target: "synaps_desktop::send", "/{name}");
        self.watch_note = Some(format!("/{name}: sent, waiting for the daemon").into());
        self.model.view_dirty = true;
        true
    }

    fn app_act(&mut self, a: AppAct, window: &mut Window, cx: &mut Context<Self>) {
        match a {
            AppAct::NewSession => self.tab_act(crate::app::TabKey::New, cx),
            AppAct::ToggleRail => self.toggle_panel(true, cx),
            AppAct::ToggleRight => self.toggle_panel(false, cx),
            AppAct::Settings => self.open_view(crate::tabs::Kind::Settings, cx),
            AppAct::Usage => self.open_view(crate::tabs::Kind::Usage, cx),
            AppAct::ThemeNext | AppAct::ThemePrev => {
                let themes = crate::palettes::THEMES;
                let now = cx.global::<crate::theme::Scheme>().choice.theme.clone();
                let at = themes.iter().position(|t| t.0 == now).unwrap_or(0);
                let n = themes.len();
                let to = if a == AppAct::ThemeNext { (at + 1) % n } else { (at + n - 1) % n };
                self.set_theme(themes[to].0, cx);
            }
            AppAct::Accent => self.open_picker(window, cx),
            AppAct::Motion => {
                let next = next_level(crate::motion::Level::current(cx));
                self.settings_pick(crate::settings_view::Pick::Motion, next.key(), cx);
            }
            AppAct::FoldAll => self.fold_all_runs(false, cx),
            AppAct::UnfoldAll => self.fold_all_runs(true, cx),
            AppAct::JumpWaiting => self.jump_to_card(window, cx),
            AppAct::Favourite => self.fav_key_pressed(window, cx),
            AppAct::Halt => self.halt(),
            AppAct::Detach => self.tab_act(crate::app::TabKey::Close, cx),
        }
    }
}

/// Motion's levels in turn: Full, Reduced, Off.
fn next_level(l: crate::motion::Level) -> crate::motion::Level {
    use crate::motion::Level;
    match l {
        Level::Full => Level::Reduced,
        Level::Reduced => Level::Off,
        Level::Off => Level::Full,
    }
}

/// GO TO's session order: needs you, running, workers, unread, opening,
/// then at rest.
fn state_rank(st: State) -> u32 {
    match st {
        State::NeedsYou => 0,
        State::Running => 1,
        State::Workers => 2,
        State::Unread => 3,
        State::Opening => 4,
        State::Idle => 5,
        State::Locked => 6,
        State::Parked => 7,
        State::Saved => 8,
    }
}

/// The palette's spring keys (fluid.md §2.1's chrome space).
const PAL_SEL: u64 = 0x7AE8 << 48;
const PAL_H: u64 = 0x7AE9 << 48;
/// The list's tallest (then it scrolls).
const LIST_MAX: f32 = 416.;
/// The box's widest, and its top under the window's.
pub(crate) const WIDTH: f32 = 640.;
pub(crate) const TOP: f32 = 72.;

impl AppView {
    /// The palette on the overlay box over the centre pane (`left`,
    /// `pane` wide), its top [`TOP`] under the window's: no scrim (the
    /// frame is the house's elevation), no enter (opened by key).
    pub(crate) fn render_palette(&self, left: f32, pane: f32, p: &Colors, cx: &Context<Self>) -> Option<impl IntoElement + use<>> {
        let pal = self.palette.as_ref()?;
        let fx = &self.fx;
        let w = WIDTH.min(pane - 48.).max(280.);
        let x = left + ((pane - w) / 2.).max(0.);
        let lines = pal.lines();
        let (ys, content) = line_y(&lines);
        let max = LIST_MAX.min((self.height - TOP - 120.).max(ROW_H * 3.));
        let target = content.min(max);
        let snap = crate::motion::SNAP_RESPONSE;
        let (h, _) = fx.spring(PAL_H, None, target, snap);
        let h = fx.snap(h);
        // The selected row's line, and the quad on its way there.
        let sel_line = lines.iter().position(|l| *l == Line::Row(pal.sel));
        let sel_y = sel_line.map(|k| ys[k]);
        let quad = sel_y.map(|y| {
            if self.palette_sel.get() != Some(pal.sel)
                && self.palette_sel.get().is_some()
                && let Some(at) = fx.spring_at(PAL_SEL)
            {
                fx.spring_jump(PAL_SEL, at, y, snap);
            }
            fx.snap(fx.spring_held(PAL_SEL, y, snap).0)
        });
        self.palette_sel.set(sel_y.map(|_| pal.sel));
        #[cfg(test)]
        self.palette_drawn.set((h, target, quad.unwrap_or(0.), sel_y.unwrap_or(0.)));
        let entries = pal.entries();
        let mut list = div().id("palette-list").relative().overflow_y_scroll().track_scroll(&self.palette_scroll).h(px(h)).w_full();
        // The quad first: under the rows it slides behind.
        if let Some(y) = quad {
            list = list.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(y))
                    .h(px(ROW_H))
                    .bg(p.card_hi)
                    .child(div().absolute().left_0().top_0().bottom_0().w(px(2.)).bg(p.you_bar)),
            );
        }
        for line in &lines {
            list = list.child(match *line {
                Line::Head(g) => {
                    let (head, jp) = g.head();
                    h_flex()
                        .h(px(HEAD_H))
                        .px(px(16.))
                        .pt(px(6.))
                        .items_center()
                        .gap(px(10.))
                        .child(display(head, size::D_S, p.dim_k))
                        .when_some(jp, |d, jp| d.child(div().font_family(fonts::JP).text_px(size::JP).text_color(p.dim_k).child(jp)))
                        .into_any_element()
                }
                Line::Row(k) => {
                    let e = &entries[pal.shown[k]];
                    let bg = if k == pal.sel { p.card_hi } else { p.card };
                    let lead = match e.lead {
                        Lead::Session(st) => crate::glyph::session_live(st, crate::glyph::Ground::Card, bg, p, fx).into_any_element(),
                        Lead::Worker(l) => {
                            let (g, _, _, f) = crate::glyph::worker(l);
                            let (_, mark) = crate::glyph::ink(f, crate::glyph::Ground::Card, p);
                            div().flex().flex_none().size(px(10.)).child(crate::glyph::spinning(fx, g, mark, bg)).into_any_element()
                        }
                        Lead::None => div().flex_none().size(px(10.)).into_any_element(),
                    };
                    let title = if e.off.is_some() { p.dim_k } else { p.prose_k };
                    h_flex()
                        .id(("palette-row", k))
                        .h(px(ROW_H))
                        .px(px(16.))
                        .gap(px(10.))
                        .items_center()
                        .whitespace_nowrap()
                        .cursor_pointer()
                        .child(lead)
                        .child(
                            h_flex()
                                .flex_1()
                                .min_w_0()
                                .gap(px(10.))
                                .overflow_hidden()
                                .child(
                                    div()
                                        .flex_none()
                                        .max_w(px(w * 0.6))
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .text_px(size::UI)
                                        .text_color(title)
                                        .child(e.title.clone()),
                                )
                                .when(!e.meta.is_empty(), |d| {
                                    d.child(mono(e.meta.clone(), size::META, p.dim_k).min_w_0().overflow_hidden().text_ellipsis())
                                }),
                        )
                        .when_some(e.key.clone(), |d, k| d.child(mono(k, size::META, p.dim_k).flex_none()))
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            cx.stop_propagation();
                            if let Some(pal) = this.palette.as_mut() {
                                pal.sel = k;
                            }
                            this.palette_pick(window, cx);
                        }))
                        .into_any_element()
                }
            });
        }
        if lines.is_empty() {
            let why = if pal.query.is_empty() { "Nothing here yet" } else { "Nothing matches" };
            list = list.child(h_flex().h(px(ROW_H)).px(px(16.)).items_center().child(mono(why, size::META, p.dim_k)));
        }
        let seg = |mode: Mode, text: &'static str| {
            let on = pal.mode == mode;
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
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    if let Some(pal) = this.palette.as_mut() {
                        pal.set_mode(mode);
                    }
                    cx.notify();
                }))
        };
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
                    .child(seg(Mode::GoTo, label::GO_TO))
                    .child(div().w(px(1.)).h_full().bg(p.frame))
                    .child(seg(Mode::Commands, label::COMMANDS)),
            )
            .child(div().flex_1())
            .child(mono("Tab", size::META, p.dim_k));
        let input = h_flex()
            .mx(px(16.))
            .mb(px(8.))
            .h(px(32.))
            .px(px(10.))
            .gap(px(10.))
            .items_center()
            .border_1()
            .border_color(p.focus_k)
            .child(crate::glyph::draw(crate::glyph::Glyph::Fold, p.you))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .font_family(fonts::MONO)
                    .text_px(13.)
                    .text_color(p.ink)
                    .child(gpui_kit::component::input::Input::new(&self.palette_input).appearance(false)),
            );
        let body = crate::overlay::frame(v_flex(), p)
            .id("palette")
            .w(px(w))
            .font_family(fonts::SANS)
            .occlude()
            .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, window, cx| this.close_palette(window, cx)))
            .child(head)
            .child(input)
            .child(div().h(px(1.)).bg(p.frame))
            .child(list)
            .child(div().h(px(8.)));
        Some(deferred(anchored().position(point(px(x), px(TOP))).child(body)).with_priority(4))
    }
}

#[cfg(test)]
mod tests {
    use super::{Act, AppAct, Entry, Group, Line, Mode, Palette, engine_of, reply_note, score};

    fn e(group: Group, title: &str, rank: u32) -> Entry {
        let mut e = Entry::new(group, Act::App(AppAct::Halt), title.to_string());
        e.rank = rank;
        e
    }

    /// polish.md P16: a prefix ranks first, then a word's start, then
    /// anywhere, then the letters in order; ties by rank (recency).
    #[test]
    fn palette_ranks_prefix_first() {
        let s = |q, t| score(q, t);
        assert!(s("rel", "relay-refresh") > s("rel", "fix relay"), "prefix over word start");
        assert!(s("rel", "fix relay") > s("rel", "unrelated"), "word start over anywhere");
        assert!(s("rel", "unrelated") > s("rel", "r-e-l"), "anywhere over subsequence");
        assert!(s("rr", "relay-refresh").is_some() && s("zz", "relay-refresh").is_none());
        assert!(s("rrf", "relay-refresh") > s("rrf", "rather rough of"), "tight runs score higher");
        let goto = vec![
            e(Group::Sessions, "unrelated", 0),
            e(Group::Sessions, "fix relay", 1),
            e(Group::Sessions, "relay-refresh", 2),
            e(Group::Sessions, "relay", 3),
            e(Group::History, "r-e-l", 0),
        ];
        let mut p = Palette::new(Mode::GoTo, goto, Vec::new());
        assert_eq!(p.shown, vec![0, 1, 2, 3, 4], "nothing typed: group order, by rank");
        p.set_query("REL ");
        let titles: Vec<&str> = p.shown.iter().map(|&i| p.goto[i].title.as_ref()).collect();
        assert_eq!(titles, ["relay", "relay-refresh", "fix relay", "unrelated", "r-e-l"]);
        assert_eq!(p.selected().map(|e| e.title.as_ref()), Some("relay"), "the best is selected");
        // A group whose best match is better goes first.
        let goto = vec![e(Group::Sessions, "unrelated", 0), e(Group::History, "relay", 0)];
        let mut p = Palette::new(Mode::GoTo, goto, Vec::new());
        p.set_query("rel");
        assert_eq!(p.lines(), vec![Line::Head(Group::History), Line::Row(0), Line::Head(Group::Sessions), Line::Row(1)]);
    }

    /// Tab flips the mode, its first row selected; the query stays.
    #[test]
    fn palette_tab_flips_mode_logic() {
        let mut p =
            Palette::new(Mode::GoTo, vec![e(Group::Sessions, "a", 0), e(Group::Sessions, "b", 1)], vec![e(Group::Commands, "c", 0)]);
        p.step(1, true);
        assert_eq!(p.sel, 1);
        p.flip();
        assert_eq!((p.mode, p.sel, p.shown.len()), (Mode::Commands, 0, 1));
        p.flip();
        assert_eq!((p.mode, p.sel), (Mode::GoTo, 0));
        p.step(-1, true);
        assert_eq!(p.sel, 1, "↑ wraps");
        p.step(5, false);
        assert_eq!(p.sel, 1, "a page stops at the end");
    }

    /// The lists change under the open palette (a session's state): the
    /// selection stays on the first row until a key moves it; then it
    /// stays on its row wherever that goes.
    #[test]
    fn a_rebuild_keeps_a_moved_selection() {
        let row = |i: usize, t: &str| {
            let mut e = Entry::new(Group::Sessions, Act::Row(crate::named::RowAction::Attach(t.to_string())), t.to_string());
            e.rank = i as u32;
            e
        };
        let rows = |order: &[&str]| order.iter().enumerate().map(|(i, t)| row(i, t)).collect::<Vec<_>>();
        let title = |p: &Palette| p.selected().map(|e| e.title.to_string());
        let mut p = Palette::new(Mode::GoTo, rows(&["a", "b"]), Vec::new());
        p.set_entries(rows(&["c", "a", "b"]), Vec::new());
        assert_eq!(title(&p).as_deref(), Some("c"), "not moved: the first");
        p.step(1, true);
        p.set_entries(rows(&["b", "c", "a"]), Vec::new());
        assert_eq!(title(&p).as_deref(), Some("a"), "moved: its row");
    }

    /// `/model x` (or `model x`) is the engine command with its argument,
    /// listed first in COMMANDS; anything else is not one.
    #[test]
    fn engine_commands_parse_with_their_argument() {
        assert_eq!(engine_of("/compact"), Some(("compact", "")));
        assert_eq!(engine_of("  model   anthropic/x "), Some(("model", "anthropic/x")));
        assert_eq!(engine_of("/compactx"), None);
        assert_eq!(engine_of("hello /compact"), None);
        let cmds = vec![e(Group::Commands, "Halt", 0), Entry::new(Group::Commands, Act::Engine("model"), "/model")];
        let mut p = Palette::new(Mode::Commands, Vec::new(), cmds);
        p.set_query("model anthropic/x");
        assert_eq!(p.selected().map(|e| e.act.clone()), Some(Act::Engine("model")));
        assert_eq!(p.arg, Some(("model", "anthropic/x".to_string())));
        assert_eq!(reply_note("context", &Ok("\n  ctx 12%\nmore".into())), "/context: ctx 12%");
        assert_eq!(reply_note("compact", &Err("refused: observing".into())), "/compact not run: refused: observing");
    }
}
