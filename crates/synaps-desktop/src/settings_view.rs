//! The Settings view: one scrolling column of groups, each saying where it
//! writes (`usage-design/spec.md` section 2). APP rows save to this
//! machine's `settings.json`; CONFIG rows go through synaps-dash's `POST
//! /api/config` only when [`crate::views::DashUi::config_gate`] proves the
//! daemon and its dash are this machine's; everything else is shown
//! read-only with its value and the reason, never as a disabled control.
//! It draws in a cached pane ([`crate::views::ViewPane`]).

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use gpui_kit::component::input::{Input, Paste};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::app::{AppView, display, mono};
use crate::dash::Fail;
use crate::fonts;
use crate::fonts::TextPx as _;
use crate::model::ConnState;
use crate::rail::State;
use crate::settings::{self, Cat};
use crate::settings_editor::Field;
use crate::settings_find::{self as find, Fields, Found, Hit};
use crate::settings_saved::{AppKey, AppValue, SettingsWriter};
use crate::theme::{self, Palette};
use crate::usage::Range;
use crate::views::{Answer, ViewCx};

/// The column's widest, the index's width, and the pane from which the
/// index shows.
const COLUMN: f32 = 760.;
const INDEX: f32 = 176.;
const INDEX_FROM: f32 = 1000.;
/// A row's control.
const CONTROL: f32 = 240.;

/// A config write on its way, or refused (the dash's message).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Write {
    Sending(String),
    Refused(String),
}

/// The new keys of `settings.json` (THIS APP).
#[derive(Clone, Debug, Default)]
pub(crate) struct AppSettings {
    /// Names a remote daemon in headers (`Welcome` carries no host name).
    pub daemon_label: Option<String>,
    /// A remote daemon's dash (a loopback forward). A token pasted with it
    /// is kept for this run only; it is saved without one.
    pub dash_url: Option<String>,
    pub usage_range: Range,
    pub usage_host: Option<String>,
    /// Split panes (`panes.split`): on by default since split panes step
    /// 6; `"panes.split": false` in the file turns them off, and
    /// `SYNAPS_DESK_SPLIT=0` (or `=1`) for a run. Off, the window is one
    /// pane, as before them. (The tests' views start off, `Default`, or
    /// on with the `split-on-tests` feature: Shady #59 M1.)
    pub split: bool,
    /// Where the model picker's favourites and logins come from
    /// (`pickers.source`, [`crate::model_list::Source`]).
    pub pickers: crate::model_list::Source,
    /// No OS notification for a waiting agent card (`notify.cards`: `false`
    /// or `"off"`; [`crate::presence`]). Unset: they notify.
    pub quiet: bool,
}

impl AppSettings {
    pub fn load(path: Option<&Path>) -> Self {
        let get = |k: &str| path.and_then(|p| theme::load_str(p, k));
        AppSettings {
            daemon_label: get("daemon_label"),
            dash_url: get("dash_url"),
            usage_range: get("usage_range").and_then(|r| Range::parse(&r)).unwrap_or_default(),
            usage_host: get("usage_host").filter(|h| h != "all"),
            split: split_env().unwrap_or_else(|| path.is_none_or(theme::load_split)),
            pickers: crate::model_list::Source::parse(path.and_then(|p| theme::load_value(p, crate::model_list::SETTING)).as_ref()),
            quiet: crate::presence::quiet(path.and_then(|p| theme::load_value(p, NOTIFY)).as_ref()),
        }
    }
}

/// The settings file's key for the cards' OS notifications.
pub(crate) const NOTIFY: &str = "notify.cards";

/// `SYNAPS_DESK_SPLIT=1` or `=0`: split panes on or off for this run,
/// whatever the file says; unset (or anything else): the file's.
pub fn split_env() -> Option<bool> {
    match std::env::var("SYNAPS_DESK_SPLIT").as_deref() {
        Ok("1") => Some(true),
        Ok("0") => Some(false),
        _ => None,
    }
}

/// What the shared edit field is editing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Edit {
    Config(&'static str),
    DaemonLabel,
    DashUrl,
}

/// What a click on a choice sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pick {
    Config(&'static str),
    TextSize,
    Motion,
    Rail,
    Right,
    /// The cards' OS notifications: on or off (D3).
    Notify,
    /// The cards' remembered choices: on or off (D4, [`crate::recall`]).
    Remember,
    Range,
    Host,
}

/// A usage host longer than this isn't offered (the writer's bound, PLAN
/// §3.6 Part 2).
pub(crate) const HOST_MAX: usize = 256;

/// The groups, in order (the index's entries; each is one child of the
/// scrolling column, after the title).
const GROUPS: [&str; 7] = [
    "C O N N E C T I O N",
    "T H I S   A P P",
    "T H I S   S E S S I O N",
    "D A E M O N   C O N F I G",
    "M O R E   D A E M O N   C O N F I G",
    "P R O V I D E R S",
    "P L U G I N S",
];

/// A shown value's hash, whether it was being sent, and when its save's
/// afterglow begins.
/// The segmented control's bar: its x and width springs, and where the
/// control was laid out (each a key space of its own).
const SEG_X: u64 = 0x7AE0 << 48;
const SEG_W: u64 = 0x7AE1 << 48;
const SEG_AT: u64 = 0x7AE2 << 48;

/// A shown value: its hash, whether it was being sent, when its afterglow
/// begins, and the THIS APP write it waits for (its generation).
type Saved = (u64, bool, Option<Instant>, Option<u64>);

#[derive(Default)]
pub(crate) struct SettingsUi {
    /// The find: its query, and what each row of the page was found as,
    /// with its hits (settings_find.rs). A row is matched when the query or
    /// its strings change; a draw reads what was found.
    pub search: RefCell<find::Search>,
    /// Each value shown (by its control): what it was, whether it was being
    /// sent, and when its save's afterglow begins (motion.md §6.8).
    pub saved: RefCell<HashMap<SharedString, Saved>>,
    pub scroll: ScrollHandle,
    pub editing: Option<Edit>,
    /// The row whose whole help shows (by key).
    pub open_help: Option<&'static str>,
    /// The open dropdown (by its control id).
    pub dropdown: Option<&'static str>,
    pub more_open: bool,
    pub writes: HashMap<&'static str, Write>,
    /// A local daemon's config as `key = value` (REFERENCE values), read
    /// off the UI thread when Settings opens (`Err`: it could not be read;
    /// its rows show `—`).
    pub reference: Option<settings::Reference>,
    /// The window's writer: what waits to be saved, what failed, and the
    /// receipts ([`crate::settings_saved`]). Its fields read as this one's.
    pub(crate) writer: SettingsWriter,
}

impl std::ops::Deref for SettingsUi {
    type Target = SettingsWriter;
    fn deref(&self) -> &SettingsWriter {
        &self.writer
    }
}

impl std::ops::DerefMut for SettingsUi {
    fn deref_mut(&mut self) -> &mut SettingsWriter {
        &mut self.writer
    }
}

/// THIS SESSION's Cache keep-warm row: its mode, read-only until the
/// client has a keep-warm call of its own (`/keepwarm` is an engine command).
/// THIS SESSION's System prompt row.
const SYSTEM_PROMPT_HELP: &str =
    "This session's system prompt (/prompt in the TUI). Applying and reloading from disk wait for the client's Set.";

const KEEPWARM_HELP: &str = "This session's keep-warm, as its status says: off, auto, or stop after <N>h. Change it with /keepwarm in the TUI. — until the daemon answers: SynapsCLI 0.13.0 and later are asked. The pin is another setting.";

/// One row's parts.
struct Row {
    id: SharedString,
    label: SharedString,
    key: &'static str,
    help: &'static str,
    control: AnyElement,
    tag: Option<&'static str>,
    under: Vec<AnyElement>,
    /// As the find took it (its hits by its number).
    found: Option<Found>,
}

fn sans(text: impl Into<SharedString>, size: f32, line: f32, color: Hsla) -> Div {
    sans_in(size, line, color).child(text.into())
}

/// [`sans`] with no text in it yet.
fn sans_in(size: f32, line: f32, color: Hsla) -> Div {
    div().text_px(size).line_height(px(line)).text_color(color)
}

/// `app::mono` with no text in it yet.
fn mono_in(size: f32, color: Hsla) -> Div {
    div().font_family(fonts::MONO).text_px(size).text_color(color).whitespace_nowrap()
}

/// A text's hit runs (one field's, from a find), at most a row's 8: those
/// within `from..to` of the field, moved to start at `at` in the text
/// drawn; sorted, an overlap merged (`StyledText` takes them so).
#[derive(Clone, Copy, Default)]
struct Runs {
    at: [(u16, u16); find::HITS_PER_ROW],
    n: u8,
}

impl Runs {
    fn of(hits: &[Hit], field: u8, from: usize, to: usize, at: usize) -> Runs {
        let mut r = Runs::default();
        for h in hits.iter().filter(|h| h.field == field) {
            let (s, e) = (usize::from(h.start), usize::from(h.end));
            if s < from || e > to || usize::from(r.n) == r.at.len() {
                continue;
            }
            r.at[usize::from(r.n)] = ((s - from + at) as u16, (e - from + at) as u16);
            r.n += 1;
        }
        r.at[..usize::from(r.n)].sort_unstable();
        let mut k = 0;
        for i in 0..usize::from(r.n) {
            let (s, e) = r.at[i];
            if k > 0 && s < r.at[k - 1].1 {
                r.at[k - 1].1 = r.at[k - 1].1.max(e);
            } else {
                r.at[k] = (s, e);
                k += 1;
            }
        }
        r.n = k as u8;
        r
    }

    fn is_empty(&self) -> bool {
        self.n == 0
    }

    fn iter(&self) -> impl Iterator<Item = std::ops::Range<usize>> + '_ {
        self.at[..usize::from(self.n)].iter().map(|&(s, e)| usize::from(s)..usize::from(e))
    }
}

/// `el` with `text` in it, its hit runs in `you` (PLAN §1.7). `said`: its
/// selector (tests) names them, `hits {name} {field} = a|b`.
fn lit(el: Div, text: SharedString, runs: Runs, you: Hsla, said: Option<(SharedString, &'static str)>) -> Div {
    if runs.is_empty() {
        return el.child(text);
    }
    let style = HighlightStyle { color: Some(you), ..Default::default() };
    let el = match said {
        Some((name, field)) => {
            let text = text.clone();
            el.debug_selector(move || format!("hits {name} {field} = {}", runs.iter().map(|r| &text[r]).collect::<Vec<_>>().join("|")))
        }
        None => el,
    };
    el.child(StyledText::new(text).with_highlights(runs.iter().map(move |r| (r, style))))
}

/// The value a session row shows.
fn ctx_window(n: u64) -> String {
    match n {
        0 => crate::usage::NONE.into(),
        200_000 => "200k".into(),
        1_000_000 => "1m".into(),
        n => format!("{} tokens", crate::usage::thousands(n)),
    }
}

impl AppView {
    /// The section a pick or a typed value lands in.
    pub(crate) fn settings_pick(&mut self, what: Pick, value: &str, cx: &mut Context<Self>) {
        self.settings.dropdown = None;
        match what {
            Pick::Config(key) => self.config_write(key, settings::to_dash(key, value), cx),
            Pick::TextSize => {
                if let Ok(n) = value.parse::<u8>() {
                    self.set_text_size(n, cx);
                }
            }
            Pick::Motion => {
                // Choosing pins it (unset follows the OS).
                if let Some(l) = crate::motion::Level::parse(value) {
                    let mut st = cx.try_global::<crate::motion::Setting>().copied().unwrap_or_default();
                    st.level = Some(l);
                    cx.set_global(st);
                    crate::motion::Level::apply(cx);
                    self.save_app(AppKey::Motion, AppValue::Str(l.key().into()), cx);
                    cx.notify();
                }
            }
            Pick::Rail | Pick::Right => {
                let rail = what == Pick::Rail;
                let folded = if rail { self.panels.rail_collapsed } else { self.panels.right_collapsed };
                if (value == "folded") != folded {
                    self.toggle_panel(rail, cx);
                }
            }
            Pick::Notify => {
                self.app_settings.quiet = value == "off";
                self.save_app(AppKey::Notify, AppValue::Str(value.into()), cx);
            }
            Pick::Remember => {
                self.set_recall(value == "on", cx);
                self.save_app(AppKey::Remember, AppValue::Str(value.into()), cx);
            }
            Pick::Range => {
                if let Some(r) = Range::parse(value) {
                    self.app_settings.usage_range = r;
                    self.save_app(AppKey::UsageRange, AppValue::Str(r.key().into()), cx);
                }
            }
            Pick::Host => {
                let h = (value != "all").then(|| value.to_string());
                self.app_settings.usage_host = h.clone();
                self.save_app(AppKey::UsageHost, AppValue::of(h), cx);
            }
        }
        self.views_changed(cx);
    }

    /// Start editing a field: the shared input takes its value and focus.
    pub(crate) fn settings_edit(&mut self, e: Edit, window: &mut Window, cx: &mut Context<Self>) {
        let value = match e {
            Edit::Config(k) => self.shown_config().and_then(|c| c.value(k)).unwrap_or("").to_string(),
            Edit::DaemonLabel => self.app_settings.daemon_label.clone().unwrap_or_default(),
            Edit::DashUrl => self.app_settings.dash_url.as_deref().map(crate::dash::strip_token).unwrap_or_default(),
        };
        // Fresh on each open (its history starts empty); a value past the
        // field's cap never opens it (its row says so).
        // STAND-IN (tests-first): any value opens.
        if value.len() > crate::settings_editor::VALUE_CAP && value.is_empty() {
            return;
        }
        self.settings.editing = Some(e);
        self.new_field(crate::settings_editor::Field::Value, &value, false, window, cx);
        self.settings_input_guard.fresh();
        self.settings_input.update(cx, |s, cx| s.focus(window, cx));
        self.views_changed(cx);
    }

    /// Enter in the edit field.
    pub(crate) fn settings_commit(&mut self, cx: &mut Context<Self>) {
        let Some(e) = self.settings.editing.take() else { return };
        let typed = self.settings_input.read(cx).value().trim().to_string();
        match e {
            Edit::Config(k) => match settings::check(k, &typed) {
                Some(why) => {
                    self.settings.writes.insert(k, Write::Refused(why));
                }
                None => self.config_write(k, settings::to_dash(k, &typed), cx),
            },
            Edit::DaemonLabel => {
                let v = (!typed.is_empty()).then_some(typed);
                self.app_settings.daemon_label = v.clone();
                self.save_app(AppKey::DaemonLabel, AppValue::of(v), cx);
            }
            Edit::DashUrl => {
                let saved = (!typed.is_empty()).then(|| crate::dash::strip_token(&typed));
                self.app_settings.dash_url = (!typed.is_empty()).then_some(typed);
                self.save_app(AppKey::DashUrl, AppValue::of(saved), cx);
                self.dash_refind(cx);
            }
        }
        self.views_changed(cx);
    }

    fn set_text_size(&mut self, n: u8, cx: &mut Context<Self>) {
        if !theme::TEXT_STEPS.contains(&n) || n == self.panels.text_size {
            return;
        }
        theme::update(cx, |c| c.text = n);
        self.text_metrics_changed();
        let mut p = self.panels;
        p.text_size = n;
        self.set_panels_pub(p, cx);
    }

    /// The find field changed: its query (at most 16 words in 256 bytes).
    pub(crate) fn settings_find(&mut self, text: String, cx: &mut Context<Self>) {
        if self.settings.search.get_mut().set(find::Query::parse(&text)) {
            self.views_changed(cx);
        }
    }

    /// A row of the page's own strings, to the find: found or not, with
    /// its rank and hits. With no query, every row.
    fn take(&self, f: &Fields<'static>) -> Option<Found> {
        self.settings.search.borrow_mut().take(f)
    }

    /// [`Self::take`] for a row of strings made for this draw (a fact, a
    /// provider, a plugin).
    fn take_made(&self, f: &Fields) -> Option<Found> {
        self.settings.search.borrow_mut().take_made(f)
    }

    /// THIS APP's row `id`, its control and what is under it built only
    /// when the find takes it.
    fn app_row(
        &self,
        rows: &mut Vec<Row>,
        id: &'static str,
        label: &'static str,
        key: &'static str,
        help: &'static str,
        build: impl FnOnce() -> (AnyElement, Vec<AnyElement>),
    ) {
        if let Some(found) = self.take(&Fields::new(label, key, help)) {
            let (control, under) = build();
            rows.push(Row { id: id.into(), label: label.into(), key, help, control, tag: None, under, found: Some(found) });
        }
    }

    /// A REFERENCE value as shown: the local config's (`default` unset),
    /// `—` for a remote daemon or a config that could not be read.
    fn reference_shown(&self, key: &str) -> String {
        match self.settings.reference.as_ref() {
            Some(settings::Reference { values: Ok(r), .. }) => settings::shown(key, r.get(key).map(String::as_str)),
            _ => crate::usage::NONE.into(),
        }
    }

    // ── controls ───────────────────────────────────────────────────────

    /// A choice of up to four: a segmented control; more: a dropdown.
    #[allow(clippy::too_many_arguments)]
    fn choice(
        &self,
        id: &'static str,
        what: Pick,
        opts: Vec<SharedString>,
        selected: Option<&str>,
        sending: Option<&str>,
        p: &Palette,
        cx: &ViewCx,
    ) -> AnyElement {
        let current = sending.or(selected);
        if opts.len() <= 4 {
            // The selected bar springs to a newly selected segment (`snap`,
            // motion.md §6.8) from the segments' places last frame; the
            // `raised` fill moves at once.
            let sel = opts.iter().position(|o| current == Some(o.as_ref()));
            let fx = &self.fx;
            // Each in a key space of its own (motion.md §2.2).
            let h = crate::app::tkey(id, 900);
            let (base, bar_x, bar_w, at) = (h, crate::motion::key(SEG_X, h), crate::motion::key(SEG_W, h), crate::motion::key(SEG_AT, h));
            let places = self.seg_places.borrow();
            let origin = places.get(&at).map(|p| p.0);
            let place = |i: usize| places.get(&crate::app::tkey(id, i)).zip(origin).map(|(&(x, w), o)| (x - o, w));
            let moved = sel.and_then(|s| fx.changed(base, s as u64)).is_some_and(|(_, at)| at == fx.now());
            let mut bar = None;
            if let Some((x, w)) = sel.and_then(place) {
                if moved && let (Some(x0), Some(w0)) = (fx.spring_at(bar_x), fx.spring_at(bar_w)) {
                    fx.spring_jump(bar_x, x0, x, crate::motion::SNAP_RESPONSE);
                    fx.spring_jump(bar_w, w0, w, crate::motion::SNAP_RESPONSE);
                }
                let ((bx, mx), (bw, mw)) =
                    (fx.spring_held(bar_x, x, crate::motion::SNAP_RESPONSE), fx.spring_held(bar_w, w, crate::motion::SNAP_RESPONSE));
                if mx || mw {
                    bar = Some((bx, bw));
                }
            }
            drop(places);
            let sink = self.seg_places.clone();
            let mut seg = h_flex()
                .relative()
                .h(px(28.))
                .border_1()
                .border_color(p.edge)
                .debug_selector(|| format!("choice {id} = {}", current.unwrap_or("none")))
                .child({
                    let sink = sink.clone();
                    canvas(
                        move |b, _, _| {
                            sink.borrow_mut().insert(at, (f32::from(b.origin.x), f32::from(b.size.width)));
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0()
                });
            for (i, o) in opts.into_iter().enumerate() {
                let on = current == Some(o.as_ref());
                let sink = sink.clone();
                let key = crate::app::tkey(id, i);
                // Where it is, for the bar's next move.
                let place = canvas(
                    move |b, _, _| {
                        sink.borrow_mut().insert(key, (f32::from(b.origin.x), f32::from(b.size.width)));
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0();
                let color = match (on, sending.is_some()) {
                    (true, true) => p.dim,
                    (true, false) => p.ink,
                    _ => p.dim,
                };
                let v = o.clone();
                seg = seg.child(
                    div()
                        .id((id, i))
                        .debug_selector(|| format!("choice {id} {o}"))
                        .relative()
                        .h_full()
                        .px(px(12.))
                        .flex()
                        .items_center()
                        .when(i > 0, |d| d.border_l_1().border_color(p.edge))
                        .child(place)
                        // Selected (design.md §5): `raised` and a 2 px you bar on top.
                        .when(on, |d| {
                            d.bg(p.raised)
                                .when(bar.is_none(), |d| d.child(div().absolute().top_0().left_0().right_0().h(px(2.)).bg(p.you_bar)))
                        })
                        .cursor_pointer()
                        .when(!on, |d| d.hover(|d| d.bg(p.raised)))
                        // Pressed (fluid.md §3.9): state, in the frame the
                        // pointer goes down.
                        .active(|d| d.bg(p.press_ch))
                        .child(mono(o, 13., color))
                        .on_click(cx.listener(move |this, _, _, cx| this.settings_pick(what, &v, cx))),
                );
            }
            // Over the segments: their fills (the new one's `raised`, a
            // hover's) never cover it on its way.
            return seg
                .children(bar.map(|(x, w)| div().absolute().top_0().left(px(x)).w(px(w)).h(px(2.)).bg(p.you_bar)))
                .into_any_element();
        }
        let open = self.settings.dropdown == Some(id);
        let shown: SharedString = current.map(|s| SharedString::from(s.to_string())).unwrap_or_else(|| "default".into());
        let mut list = v_flex().w(px(CONTROL)).border_1().border_color(p.edge).bg(p.canvas);
        if open {
            for (i, o) in opts.into_iter().enumerate() {
                let on = current == Some(o.as_ref());
                let v = o.clone();
                list = list.child(
                    h_flex()
                        .id((id, 100 + i))
                        .debug_selector(|| format!("option {id} {o}"))
                        .h(px(26.))
                        .px(px(10.))
                        .items_center()
                        .cursor_pointer()
                        .when(on, |d| d.bg(p.raised))
                        .hover(|d| d.bg(p.raised))
                        .child(mono(o, 13., if on { p.ink } else { p.prose }))
                        .on_click(cx.listener(move |this, _, _, cx| this.settings_pick(what, &v, cx))),
                );
            }
        }
        v_flex()
            .items_end()
            .child(
                h_flex()
                    .id(id)
                    .debug_selector(move || format!("dropdown {id}"))
                    .w(px(CONTROL))
                    .h(px(28.))
                    .px(px(10.))
                    .items_center()
                    .border_1()
                    .border_color(if open { p.focus } else { p.edge })
                    .cursor_pointer()
                    .child({
                        let k = self.saved_glow(&SharedString::from(id), &shown, sending.is_some());
                        let said = shown.clone();
                        mono(shown, 13., if sending.is_some() { p.dim } else { crate::motion::mix(p.prose, p.ink, k) })
                            .flex_1()
                            .debug_selector(move || format!("choice {id} = {said}"))
                    })
                    .child(mono(if open { "↑" } else { "↓" }, 12., p.dim))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.settings.dropdown = if this.settings.dropdown == Some(id) { None } else { Some(id) };
                        this.views_changed(cx);
                    })),
            )
            .when(open, |d| d.child(list))
            .into_any_element()
    }

    /// A number or text field: its value in a box; click to edit (Enter
    /// saves, Esc leaves it).
    /// A shown value's afterglow once saved (motion.md §6.8: the
    /// confirmation, no toast): a THIS APP value when the write that saved
    /// it finished (none if it failed), a daemon one when its write is
    /// answered. Never on first sight: what changed while Settings was not
    /// shown is history (the map is cleared when it hides). Its `k`.
    /// The THIS APP key of control `id` ("usage-range", "field-dash-url")
    /// is waiting to be written, or being written.
    fn app_pending(&self, id: &SharedString) -> bool {
        let key = id.strip_prefix("field-").unwrap_or(id).replace('-', "_");
        let key = key.as_str();
        self.settings.app_dirty.contains_key(key) || self.settings.app_writing.as_ref().is_some_and(|(batch, _)| batch.contains_key(key))
    }

    pub(crate) fn saved_glow(&self, id: &SharedString, value: &str, sending: bool) -> f32 {
        let hash = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            value.hash(&mut h);
            h.finish()
        };
        let now = self.fx.now();
        let mut saved = self.settings.saved.borrow_mut();
        let wrote = self.settings.app_wrote;
        let since = match saved.get(id).copied() {
            Some((was, was_sending, _, _)) if (was, was_sending) != (hash, sending) => {
                let (since, wait) = match (sending, was_sending) {
                    (true, _) => (None, None),
                    (false, true) => (Some(now), None),
                    // Changed, not by a send: this app's own key waits for
                    // its write; anything else (another client, the
                    // daemon) glows as it lands.
                    (false, false) if self.app_pending(id) => (None, Some(self.settings.app_gen)),
                    (false, false) => (Some(now), None),
                };
                saved.insert(id.clone(), (hash, sending, since, wait));
                since
            }
            Some((h, s, None, Some(g))) => match wrote.filter(|&(w, _)| w >= g) {
                Some((_, at)) => {
                    saved.insert(id.clone(), (h, s, Some(at), None));
                    Some(at)
                }
                None => None,
            },
            Some((_, _, since, _)) => since,
            None => {
                saved.insert(id.clone(), (hash, sending, None, None));
                None
            }
        };
        drop(saved);
        self.fx.afterglow(since)
    }

    fn field(&self, e: Edit, value: SharedString, unit: &'static str, sending: bool, p: &Palette, cx: &ViewCx) -> AnyElement {
        let editing = self.settings.editing == Some(e);
        let id: SharedString = match e {
            Edit::Config(k) => format!("field-{k}").into(),
            Edit::DaemonLabel => "field-daemon-label".into(),
            Edit::DashUrl => "field-dash-url".into(),
        };
        // Longer than the field takes: its length, and where to edit it.
        // STAND-IN (tests-first): a value of any length is shown.
        let long = !editing && value.len() > crate::settings_editor::VALUE_CAP && value.is_empty();
        let value: SharedString = match long {
            true => format!("{} bytes: longer than this app edits; use /settings in the TUI", crate::usage::thousands(value.len() as u64))
                .into(),
            false => value,
        };
        let body = if editing {
            div()
                .flex_1()
                .min_w_0()
                .font_family(crate::fonts::MONO)
                .text_px(13.)
                .capture_action(cx.listener(|this, _: &Paste, window, cx| this.field_paste(Field::Value, window, cx)))
                .child(Input::new(&self.settings_input).appearance(false))
                .into_any_element()
        } else {
            let k = self.saved_glow(&id, &value, sending);
            let color = if sending { p.dim } else { crate::motion::mix(p.prose, p.ink, k) };
            let said = (id.clone(), value.clone());
            mono(value, 13., color)
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .debug_selector(move || format!("{} = {}", said.0, said.1))
                .into_any_element()
        };
        let boxed = h_flex()
            .debug_selector(|| id.to_string())
            .id(id)
            .w(px(CONTROL))
            .h(px(28.))
            .px(px(10.))
            .gap(px(8.))
            .items_center()
            .border_1()
            .border_color(if editing { p.focus } else { p.edge })
            .when(!editing && !long, |d| d.cursor_text())
            .child(body)
            .when(!unit.is_empty(), |d| d.child(mono(unit, 12., p.dim)))
            .on_click(cx.listener(move |this, _, window, cx| {
                if this.settings.editing != Some(e) {
                    this.settings_edit(e, window, cx);
                }
            }));
        // Under the open field: a paste refused, or its history cleared.
        match self.field_note(Field::Value).filter(|_| editing) {
            Some(n) => v_flex()
                .items_end()
                .gap(px(4.))
                .child(h_flex().child(boxed))
                .child(mono(n.clone(), 12., p.dim).debug_selector(move || format!("field-note {n}")))
                .into_any_element(),
            None => boxed.into_any_element(),
        }
    }

    /// A read-only value: dim text, no border, then the rail's LOCKED glyph
    /// (at `locked <key> = <value>`).
    fn locked(key: &str, value: impl Into<SharedString>, p: &Palette) -> AnyElement {
        let value = value.into();
        Self::locked_div(value.clone(), p).debug_selector(move || format!("locked {key} = {value}")).into_any_element()
    }

    fn locked_div(value: SharedString, p: &Palette) -> Div {
        h_flex()
            .gap(px(10.))
            .items_center()
            .justify_end()
            .max_w(px(CONTROL + 120.))
            .child(mono(value, 13., p.dim).overflow_hidden().text_ellipsis())
            .child(crate::glyph::session_mark(State::Locked, crate::glyph::Ground::Canvas, p))
    }

    /// A THIS SESSION row's value, read-only, at `session-<key>-value
    /// <value>` (the right panel's cells are named the same way).
    fn session_locked(key: &'static str, value: String, p: &Palette) -> AnyElement {
        let selector = format!("session-{key}-value {value}");
        Self::locked_div(value.into(), p).debug_selector(move || selector).into_any_element()
    }

    /// The control for a CONFIG key: editable through the dash, else its
    /// value read-only (`—` when it cannot be read).
    fn config_control(&self, key: &'static str, writable: bool, p: &Palette, cx: &ViewCx) -> AnyElement {
        let snap = self.shown_config().filter(|c| c.exposes(key));
        let raw = snap.and_then(|c| c.value(key));
        let sending = match self.settings.writes.get(key) {
            Some(Write::Sending(v)) => Some(v.as_str()),
            _ => None,
        };
        if snap.is_none() {
            return Self::locked(key, self.reference_shown(key), p);
        }
        let shown = settings::shown(key, raw);
        if !writable {
            return Self::locked(key, shown, p);
        }
        let id: &'static str = settings::def(key).map_or("config", |d| d.key);
        let control = match settings::config_choices(key) {
            Some(opts) => {
                self.choice(id, Pick::Config(key), opts.iter().map(|o| SharedString::from(*o)).collect(), Some(&shown), sending, p, cx)
            }
            None => {
                self.field(Edit::Config(key), sending.unwrap_or(&shown).to_string().into(), settings::unit(key), sending.is_some(), p, cx)
            }
        };
        let at = sending.is_none().then(|| self.daemon_saved_at(key, raw)).flatten();
        let at = at.map(|r| format!("saved {}", crate::settings_saved::when(r.at_ms, crate::settings_saved::now_ms())));
        Self::saved_beside(control, key, at, p)
    }

    /// Lines under a CONFIG row: what its editor takes when the row's help
    /// names more ([`settings::editor_note`], at `config-note-<key>`), the
    /// refusal, and when its change takes effect ([`crate::settings_effect::lines`], at
    /// `effect <key> = <lines>`; a wait in warn, at `warn <key>`).
    fn config_under(&self, key: &'static str, writable: bool, p: &Palette) -> Vec<AnyElement> {
        let mut out = Vec::new();
        let editor = writable && self.shown_config().is_some_and(|c| c.exposes(key));
        if let Some(note) = settings::editor_note(key).filter(|_| editor) {
            let selector = format!("config-note-{key}");
            out.push(mono(note, 12., p.dim).whitespace_normal().debug_selector(move || selector).into_any_element());
        }
        if let Some(Write::Refused(m)) = self.settings.writes.get(key) {
            out.push(mono(m.clone(), 12., p.err).whitespace_normal().debug_selector(|| format!("refused {key} = {m}")).into_any_element());
        }
        if let Some(c) = self.shown_config().filter(|c| c.exposes(key)) {
            let wrote = self.daemon_saved_at(key, c.value(key)).is_some();
            let seen = crate::settings_effect::Observed { idle_exit_running: c.idle_exit_running };
            let lines =
                crate::settings_effect::lines(key, self.daemon_version(), c.value(key), &seen, wrote, self.settings.open_help == Some(key));
            if !lines.is_empty() {
                let said = lines.iter().map(|(l, _)| l.as_str()).collect::<Vec<_>>().join(" | ");
                let lines = lines.into_iter().map(|(text, warn)| {
                    let line = mono(text, 12., if warn { p.warn } else { p.dim }).whitespace_normal();
                    if warn { line.debug_selector(move || format!("warn {key}")) } else { line }
                });
                out.push(v_flex().gap(px(2.)).debug_selector(move || format!("effect {key} = {said}")).children(lines).into_any_element());
            }
            // Its save's time is this run's only: its write failed.
            if let Some(n) =
                self.settings.receipts_note.as_ref().filter(|_| self.daemon_saved_at(key, c.value(key)).is_some_and(|r| r.failed()))
            {
                let why = n.why.clone();
                out.push(
                    mono(n.why.to_string(), 12., p.err)
                        .whitespace_normal()
                        .debug_selector(move || format!("not-kept {key} = {why}"))
                        .into_any_element(),
                );
            }
        }
        out
    }

    /// Under a THIS APP row: its key's failed save (the app values' write,
    /// or the panels'), `not saved: … · Retry`.
    fn not_saved(&self, key: &'static str, p: &Palette, cx: &ViewCx) -> Option<AnyElement> {
        let bit = crate::settings_saved::AppKeySet::default().with(AppKey::of(key)?).0;
        let n = [&self.settings.app_note, &self.settings.panels_note].into_iter().flatten().find(|n| n.keys & bit != 0)?;
        let why = n.why.clone();
        let retry = div()
            .id(SharedString::from(format!("retry-{key}")))
            .debug_selector(move || format!("retry {key}"))
            .cursor_pointer()
            .child(mono("Retry", 12., p.you))
            .on_click(cx.listener(|this, _, _, cx| this.retry_saves(cx)));
        let line = h_flex()
            .gap(px(6.))
            .items_start()
            .debug_selector(move || format!("not-saved {key} = {why}"))
            .child(mono(n.why.to_string(), 12., p.err).whitespace_normal())
            .child(mono("·", 12., p.dim))
            .child(retry);
        Some(line.into_any_element())
    }

    /// `control` with `saved 13:58` before it (at `saved-at <key> = …`),
    /// while the value it shows is the one this app saved.
    fn saved_beside(control: AnyElement, key: &'static str, at: Option<String>, p: &Palette) -> AnyElement {
        let Some(at) = at else { return control };
        let said = at.clone();
        h_flex()
            .gap(px(10.))
            .items_center()
            .child(mono(at, 12., p.dim).debug_selector(move || format!("saved-at {key} = {said}")))
            .child(control)
            .into_any_element()
    }

    /// A daemon row's tag: when its change takes effect, once the dash's
    /// config is read.
    fn applies(&self, key: &str) -> Option<&'static str> {
        self.shown_config().filter(|c| c.exposes(key))?;
        crate::settings_effect::effect_of(key, self.daemon_version()).map(crate::settings_effect::Effect::word)
    }

    /// The connected daemon's `Welcome.daemon_version` (empty: none).
    fn daemon_version(&self) -> &str {
        self.model.daemon.as_ref().map_or("", |d| d.version.as_str())
    }

    // ── layout ─────────────────────────────────────────────────────────

    fn render_row(&self, r: Row, first: bool, p: &Palette, cx: &ViewCx) -> AnyElement {
        let open = self.settings.open_help == Some(r.key);
        let search = self.settings.search.borrow();
        let hits = r.found.map_or(&[][..], |f| search.hits_of(f.row));
        // Its help: all of it when open; else the first sentence, or the
        // sentence that holds the find's first hit past it (with `…`).
        let lead = settings::first_sentence(r.help);
        let past = hits.iter().filter(|h| h.field == find::HELP).map(|h| usize::from(h.start)).min().filter(|&at| at >= lead.len());
        let (help, help_runs): (SharedString, Runs) = match past {
            _ if open => (r.help.into(), Runs::of(hits, find::HELP, 0, r.help.len(), 0)),
            Some(at) => {
                let s = settings::sentence_at(r.help, at);
                (format!("… {}", &r.help[s.clone()]).into(), Runs::of(hits, find::HELP, s.start, s.end, "… ".len()))
            }
            None => (lead.into(), Runs::of(hits, find::HELP, 0, lead.len(), 0)),
        };
        let (label_runs, key_runs) = (Runs::of(hits, find::LABEL, 0, r.label.len(), 0), Runs::of(hits, find::KEY, 0, r.key.len(), 0));
        drop(search);
        let name: SharedString = if r.key.is_empty() { r.id.clone() } else { r.key.into() };
        let key = r.key;
        let more = r.help.len() > settings::first_sentence(r.help).len();
        let line = h_flex()
            .w_full()
            .gap(px(24.))
            .items_start()
            .child(
                v_flex()
                    .id("help")
                    .debug_selector(move || format!("help {key}"))
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .when(more, |d| d.cursor_pointer())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.settings.open_help = if this.settings.open_help == Some(key) { None } else { Some(key) };
                        this.views_changed(cx);
                    }))
                    .child(
                        h_flex()
                            .gap(px(10.))
                            .items_baseline()
                            .when(!label_runs.is_empty() || !key_runs.is_empty(), |d| {
                                let name = name.clone();
                                d.debug_selector(move || format!("hit-{name}"))
                            })
                            .child(lit(sans_in(14., 22., p.ink), r.label, label_runs, p.you, Some((name.clone(), "label"))))
                            .when(!key.is_empty(), |d| d.child(lit(mono_in(12., p.dim), key.into(), key_runs, p.you, Some((name, "key"))))),
                    )
                    .when(!r.help.is_empty(), |d| {
                        // Its selector says the text it was handed (tests).
                        let said = help.clone();
                        d.child(
                            lit(sans_in(13., 20., p.dim), help, help_runs, p.you, None)
                                .whitespace_normal()
                                .debug_selector(move || format!("help-text {key} = {said}")),
                        )
                    })
                    .when_some(r.tag, |d, t| d.child(mono(t, 12., p.dim).mt(px(2.)).debug_selector(move || format!("tag {key} = {t}")))),
            )
            .child(div().flex_none().flex().justify_end().child(r.control));
        // Under it, full width: a session row's default, a refusal, when it
        // takes effect.
        v_flex()
            .debug_selector(|| format!("row {}", r.id))
            .id(r.id)
            .w_full()
            .min_h(px(44.))
            .py(px(12.))
            .gap(px(2.))
            .when(!first, |d| d.border_t_1().border_color(p.hair))
            .child(line)
            .children(r.under)
            .into_any_element()
    }

    fn group_head(&self, ix: usize, title: SharedString, line: SharedString, p: &Palette, cx: &ViewCx) -> Div {
        let more = ix == 4;
        let open = self.settings.more_open || !self.settings.search.borrow().query().is_empty();
        v_flex()
            .pt(px(40.))
            .pb(px(8.))
            .gap(px(4.))
            .child(
                h_flex()
                    .id(("group-head", ix))
                    .debug_selector(move || format!("group {ix}"))
                    .gap(px(10.))
                    .items_baseline()
                    .when(more, |d| {
                        d.cursor_pointer().on_click(cx.listener(|this, _, _, cx| {
                            this.settings.more_open = !this.settings.more_open;
                            this.views_changed(cx);
                        }))
                    })
                    .when(more, |d| d.child(mono(if open { "−" } else { "+" }, 13., p.dim)))
                    .child(display(title, fonts::size::D_M, p.header)),
            )
            .child({
                let said = line.clone();
                mono(line, 12., p.dim).whitespace_normal().debug_selector(move || format!("group-line {ix} = {said}"))
            })
    }

    fn sub_head(title: &'static str, p: &Palette) -> AnyElement {
        div().pt(px(16.)).pb(px(4.)).child(display(title, 12., p.dim)).into_any_element()
    }

    /// The Settings view, `pane` px wide. Reads, never changes, the app.
    pub(crate) fn render_settings(&self, pane: f32, p: &Palette, cx: &ViewCx) -> AnyElement {
        let col = (pane - 48.).clamp(320., COLUMN);
        let index = pane >= INDEX_FROM;
        let left = if index { ((pane - col) / 2.).max(INDEX + 48.) } else { ((pane - col) / 2.).max(24.) };
        let (on, said, note) = {
            let s = self.settings.search.borrow();
            (!s.query().is_empty(), s.query().text().to_string(), s.query().note())
        };
        let wrap = |inner: AnyElement| h_flex().w_full().pl(px(left)).child(div().w(px(col)).flex_none().child(inner)).into_any_element();
        let mut blocks: Vec<AnyElement> = Vec::with_capacity(8);
        // Title and find.
        blocks.push(wrap(
            h_flex()
                .pt(px(28.))
                .gap(px(24.))
                .items_center()
                .child(display("S E T T I N G S", fonts::size::D_L, p.header))
                .child(div().flex_1())
                .child(
                    div()
                        .relative()
                        .child(
                            h_flex()
                                .w(px(CONTROL))
                                .h(px(28.))
                                .px(px(10.))
                                .items_center()
                                .border_1()
                                .border_color(p.edge)
                                .debug_selector(|| "settings-find".into())
                                .child(
                                    div()
                                        .flex_1()
                                        .font_family(crate::fonts::MONO)
                                        .text_px(13.)
                                        .capture_action(
                                            cx.listener(|this, _: &Paste, window, cx| this.field_paste(Field::Find, window, cx)),
                                        )
                                        .child(Input::new(&self.settings_find).appearance(false)),
                                ),
                        )
                        // A query cut to its first 16 words or 256 bytes says
                        // so, under the field (taking no room from the page);
                        // so does a paste refused, or its history cleared.
                        .when_some(self.field_note(Field::Find).map(SharedString::from).or(note.map(SharedString::from)), |d, n| {
                            d.child(
                                mono(n.clone(), 12., p.dim)
                                    .absolute()
                                    .top(px(32.))
                                    .right(px(0.))
                                    .debug_selector(move || format!("settings-find-note {n}")),
                            )
                        }),
                )
                .into_any_element(),
        ));
        let gate = self.config_gate();
        let walk = || {
            [
                self.group_connection(p),
                self.group_app(p, cx),
                self.group_session(p, cx, gate.is_ok()),
                self.group_config(p, cx, &gate),
                self.group_more(p),
                self.group_providers(p),
                self.group_plugins(p),
            ]
        };
        // The find's walk over the rows (settings_find.rs): a row is matched
        // only when the query or its strings changed. When the walk finds
        // the other way right (rows as typed came or went), the page walks
        // again, from what was found.
        self.settings.search.borrow_mut().begin();
        let mut groups = walk();
        if self.settings.search.borrow_mut().end() {
            self.settings.search.borrow_mut().begin();
            groups = walk();
            self.settings.search.borrow_mut().end();
        }
        let mut shown_groups = Vec::new();
        for (ix, (line, rows)) in groups.into_iter().enumerate() {
            if on && rows.is_empty() {
                continue;
            }
            shown_groups.push(ix);
            let mut g = self.group_head(ix, GROUPS[ix].into(), line, p, cx);
            let folded = ix == 4 && !self.settings.more_open && !on;
            if !folded {
                g = g.children(rows);
            }
            blocks.push(wrap(g.into_any_element()));
        }
        if on && shown_groups.is_empty() {
            let nothing = sans(format!("Nothing matches “{said}”."), 13., 20., p.dim).pt(px(40.));
            blocks.push(wrap(nothing.debug_selector(|| format!("settings-nothing {said}")).into_any_element()));
        }
        blocks.push(div().h(px(64.)).into_any_element());
        // The group at the top of the view (child 0 is the title). A scroll
        // draws the pane again by itself (GPUI notifies the scrolled view).
        let top = self.settings.scroll.top_item().saturating_sub(1);
        let in_view = shown_groups.get(top.min(shown_groups.len().saturating_sub(1))).copied();
        let column = v_flex().id("settings-scroll").size_full().overflow_y_scroll().track_scroll(&self.settings.scroll).children(blocks);
        let bar = crate::scrollbar::Bar::new(self.settings.scroll.clone(), &self.bars.settings);
        let mut root = div().relative().size_full().bg(p.canvas).child(column).child(bar);
        if index {
            let mut list = v_flex().absolute().top(px(84.)).left(px(left - INDEX - 24.)).w(px(INDEX)).gap(px(2.));
            for (pos, &ix) in shown_groups.iter().enumerate() {
                let on = in_view == Some(ix);
                list = list.child(
                    // The group in view: a you bar on its left (design.md §5).
                    div()
                        .id(("settings-index", ix))
                        .debug_selector(move || format!("settings-index {ix}"))
                        .relative()
                        .py(px(5.))
                        .pl(px(10.))
                        .cursor_pointer()
                        .when(on, |d| d.child(crate::glyph::bar(p.you_mark).debug_selector(move || format!("settings-index-bar {ix}"))))
                        .child(display(GROUPS[ix], 12., if on { p.ink } else { p.dim }))
                        .hover(|d| d.text_color(p.ink))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.settings.scroll.scroll_to_top_of_item(pos + 1);
                            this.views_changed(cx);
                        })),
                );
            }
            root = root.child(list);
        }
        root.into_any_element()
    }

    // ── the groups: (header line, rows) ────────────────────────────────

    /// A fact: its label and value, the find's hits in them (`hits`).
    fn info_row(label: &'static str, value: impl Into<SharedString>, first: bool, hits: &[Hit], p: &Palette) -> AnyElement {
        let value = value.into();
        let (lr, vr) = (Runs::of(hits, find::LABEL, 0, label.len(), 0), Runs::of(hits, find::VALUE, 0, value.len(), 0));
        h_flex()
            .debug_selector(|| format!("fact {label} = {value}"))
            .w_full()
            .min_h(px(36.))
            .py(px(8.))
            .gap(px(24.))
            .items_center()
            .when(!first, |d| d.border_t_1().border_color(p.hair))
            .child(lit(sans_in(14., 22., p.ink), label.into(), lr, p.you, Some((label.into(), "label"))).w(px(180.)).flex_none())
            .child(lit(mono_in(13., p.prose), value, vr, p.you, Some((label.into(), "value"))).flex_1().min_w_0().whitespace_normal())
            .into_any_element()
    }

    fn group_connection(&self, p: &Palette) -> (SharedString, Vec<AnyElement>) {
        let d = self.model.daemon.as_ref();
        let socket = self.dash.socket.as_ref().map(|s| s.display().to_string()).unwrap_or_else(|| crate::usage::NONE.into());
        let kind = match (&self.dash.local, d) {
            (_, None) if self.synthetic => "offline (synthetic sessions)".to_string(),
            (_, None) => "not connected".to_string(),
            (Some(Ok(())), _) => "local daemon".to_string(),
            (None, _) => "checking where it runs".to_string(),
            (Some(Err(_)), _) => match &self.app_settings.daemon_label {
                Some(l) => format!("{l} · remote daemon"),
                None => "remote daemon".to_string(),
            },
        };
        let place = match &self.dash.local {
            Some(Ok(())) => "this machine's registry has its pid and socket".to_string(),
            Some(Err(why)) => why.clone(),
            None => crate::usage::NONE.into(),
        };
        // Nit 6: the last Welcome's daemon, while not connected to it, is
        // what it was, not what it is.
        let kind = match (&self.model.conn, d) {
            (ConnState::Connected(_), _) | (_, None) => kind,
            (ConnState::Off(e), _) => format!("offline ({e}) · last: {kind}"),
            (ConnState::Incompatible(e), _) => format!("{e} · last: {kind}"),
            (ConnState::Connecting | ConnState::Reconnecting, _) => format!("reconnecting · last: {kind}"),
        };
        let line = match d {
            Some(d) => format!("{kind} · v{} · pid {} · {}", d.version, d.pid, crate::named::home_short(&socket)),
            None => kind.clone(),
        };
        let dash = match &self.dash.found {
            None => "not found: no URL file, --dash, $SYNAPS_DASH_URL or dash_url".to_string(),
            Some(Err(e)) => format!("not used: {e}"),
            Some(Ok((t, src))) => {
                let state = match &self.dash.answer {
                    Answer::Up => "answers".to_string(),
                    Answer::Unknown => "not asked yet".to_string(),
                    Answer::Down(e) => self.fail_text(e),
                };
                let serves = match &self.dash.serves {
                    Some(Ok(i)) => format!(" · serves pid {} ({})", i.pid, crate::dash::profile_name(i.profile.as_deref())),
                    _ => String::new(),
                };
                format!("{} · {state}{serves} · from {}", t.shown(), src.describe())
            }
        };
        let sources = self
            .usage
            .board
            .as_ref()
            .map(|b| b.sources.iter().map(|s| s.text.clone()).collect::<Vec<_>>().join(" · "))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "open Usage to read them".into());
        let rows: Vec<(&'static str, String)> = vec![
            ("Daemon", kind),
            ("Where", place),
            ("Version", d.map(|d| d.version.clone()).unwrap_or_else(|| crate::usage::NONE.into())),
            ("Process", d.map(|d| d.pid.to_string()).unwrap_or_else(|| crate::usage::NONE.into())),
            ("Profile", d.map(|d| d.profile.clone().unwrap_or_else(|| "default".into())).unwrap_or_else(|| crate::usage::NONE.into())),
            ("Protocol", synaps_client::protocol::PROTOCOL_VERSION.to_string()),
            ("Socket", socket),
            ("synaps-dash", dash),
            ("Telemetry sources", sources),
        ];
        let mut rows: Vec<(Found, &'static str, String)> =
            rows.into_iter().filter_map(|(l, v)| self.take_made(&Fields::fact(l, &v)).map(|f| (f, l, v))).collect();
        rows.sort_by_key(|(f, ..)| f.rank);
        let search = self.settings.search.borrow();
        let rows = rows.into_iter().enumerate().map(|(i, (f, l, v))| Self::info_row(l, v, i == 0, search.hits_of(f.row), p)).collect();
        (line.into(), rows)
    }

    fn group_app(&self, p: &Palette, cx: &ViewCx) -> (SharedString, Vec<AnyElement>) {
        let path =
            theme::settings_path().map(|p| crate::named::home_short(&p.display().to_string())).unwrap_or_else(|| "no config folder".into());
        let scheme = cx.global::<theme::Scheme>();
        let mut rows: Vec<Row> = Vec::new();
        let pinned = |src: theme::Source, flag: &str, env: &str| match src {
            theme::Source::Flag => Some(format!("set by {flag} for this run")),
            theme::Source::Env => Some(format!("set by ${env}")),
            _ => None,
        };
        let open_picker = |id: &'static str| {
            // The outlined button (design.md §8.12).
            div()
                .id(id)
                .debug_selector(move || id.into())
                .h(px(28.))
                .px(px(16.))
                .flex()
                .items_center()
                .border_1()
                .border_color(p.edge)
                .cursor_pointer()
                .hover(|d| d.bg(p.raised))
                .child(display("C H O O S E", fonts::size::D_S, p.prose))
                .on_click(cx.listener(|this, _, window, cx| this.open_picker(window, cx)))
        };
        let theme_value = if self.theme_follows_config {
            format!("{} · follows the Synaps config", scheme.choice.theme)
        } else {
            scheme.choice.theme.clone()
        };
        self.app_row(
            &mut rows,
            "app-theme",
            "Theme",
            "theme",
            "One of the Synaps TUI's themes. Following the Synaps config shows the theme its `theme =` names.",
            || {
                let control = match pinned(self.theme_source, "--theme", theme::ENV_THEME) {
                    Some(why) => Self::locked("theme", format!("{} · {why}", scheme.choice.theme), p),
                    None => h_flex()
                        .gap(px(12.))
                        .items_center()
                        .child({
                            let shown = SharedString::from(theme_value);
                            let said = shown.clone();
                            mono(shown, 13., p.prose).debug_selector(move || format!("value theme = {said}"))
                        })
                        .child(open_picker("app-theme-pick"))
                        .into_any_element(),
                };
                (control, Vec::new())
            },
        );
        let accent = scheme.choice.accent.map(theme::to_hex).unwrap_or_else(|| "none (the theme's)".into());
        self.app_row(
            &mut rows,
            "app-accent",
            "Accent",
            "accent",
            "Replaces the theme's “you” and focus colours; never a state.",
            || {
                let control = match pinned(self.accent_source, "--accent", theme::ENV_ACCENT) {
                    Some(why) => Self::locked("accent", format!("{accent} · {why}"), p),
                    None => h_flex()
                        .gap(px(12.))
                        .items_center()
                        .child({
                            let shown = SharedString::from(accent);
                            let said = shown.clone();
                            mono(shown, 13., p.prose).debug_selector(move || format!("value accent = {said}"))
                        })
                        .child(open_picker("app-accent-pick"))
                        .into_any_element(),
                };
                (control, Vec::new())
            },
        );
        let size = self.panels.text_size.to_string();
        self.app_row(&mut rows, "app-text", "Text size", "text_size", "The centre pane's text, in px (Ctrl+= and Ctrl+- step it).", || {
            let control = self.choice(
                "text-size",
                Pick::TextSize,
                theme::TEXT_STEPS.iter().map(|s| SharedString::from(s.to_string())).collect(),
                Some(&size),
                None,
                p,
                cx,
            );
            (control, Vec::new())
        });
        let motion = cx.try_global::<crate::motion::Setting>().copied().unwrap_or_default();
        let level = crate::motion::Level::current(cx);
        self.app_row(
            &mut rows,
            "app-motion",
            "Motion",
            "motion",
            "Full, reduced (no movement, short fades) or off (nothing animates). Follows the system until you choose.",
            || {
                let control = match motion.lock {
                    Some(crate::motion::Lock::Flag) => Self::locked("motion", format!("{} · set by --motion for this run", level.key()), p),
                    Some(crate::motion::Lock::NoBootFx) => {
                        Self::locked("motion", format!("off · set by ${}", crate::motion::ENV_NO_BOOT_FX), p)
                    }
                    None => self.choice(
                        "motion",
                        Pick::Motion,
                        crate::motion::Level::ALL.iter().map(|l| SharedString::from(l.key())).collect(),
                        Some(level.key()),
                        None,
                        p,
                        cx,
                    ),
                };
                (control, Vec::new())
            },
        );
        self.app_row(
            &mut rows,
            "app-notify",
            "Card notifications",
            NOTIFY,
            "An OS notification when an agent waits on a card while the window, or its session, is not in front.",
            || {
                let control = self.choice(
                    NOTIFY,
                    Pick::Notify,
                    vec!["on".into(), "off".into()],
                    Some(if self.app_settings.quiet { "off" } else { "on" }),
                    None,
                    p,
                    cx,
                );
                (control, Vec::new())
            },
        );
        self.app_row(&mut rows, "app-recall", "Remember card choices", crate::recall::SETTING, "When an agent asks the same card question again in the same folder, the card starts with your last choice highlighted, marked “last time”. The highlight is only a starting point: nothing is ticked or sent for you, and Enter sends what's ticked (on a one-choice card, the option highlighted). Only which option you picked is kept, on this machine, never a note or a field's text.", || {
            // Forget while anything may be there: off (the file unread), a
            // choice remembered, the file there (an unreadable one too), or a
            // removal or a sweep that failed, said under the row.
            let forget: AnyElement = if !self.recall.offers_forget() {
                mono("nothing remembered", 12., p.dim).debug_selector(|| "recall-none".into()).into_any_element()
            } else {
                div()
                    .id("recall-forget")
                    .debug_selector(|| "recall-forget".into())
                    .cursor_pointer()
                    .child(mono("forget remembered choices", 12., p.dim))
                    .hover(|d| d.text_color(p.ink))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.forget_card_choices(cx);
                        this.views_changed(cx);
                    }))
                    .into_any_element()
            };
            let control = h_flex()
                .gap(px(12.))
                .items_center()
                .child(forget)
                .child(self.choice(
                    crate::recall::SETTING,
                    Pick::Remember,
                    vec!["on".into(), "off".into()],
                    Some(if self.recall.on() { "on" } else { "off" }),
                    None,
                    p,
                    cx,
                ))
                .into_any_element();
            (control, self
                .recall
                .not_removed()
                .map(|why| {
                    mono(format!("couldn't remove {why}"), 12., p.err)
                        .whitespace_normal()
                        .debug_selector(|| "recall-failed".into())
                        .into_any_element()
                })
                .into_iter()
                .collect())
        });
        for (rail, label, key, id) in
            [(true, "Sessions rail", "rail_collapsed", "app-rail"), (false, "Right panel", "right_collapsed", "app-right")]
        {
            let help = if rail {
                "Shown or folded to its strip (Ctrl+B); its dragged width."
            } else {
                "Shown or folded to its strip (Ctrl+Alt+B); its dragged width."
            };
            self.app_row(&mut rows, id, label, key, help, || {
                let folded = if rail { self.panels.rail_collapsed } else { self.panels.right_collapsed };
                let width = if rail { self.panels.rail_width } else { self.panels.right_width };
                let mut control = h_flex().gap(px(12.)).items_center();
                if width.is_some() {
                    control = control.child(
                        div()
                            .id(if rail { "rail-reset" } else { "right-reset" })
                            .debug_selector(move || if rail { "rail-reset" } else { "right-reset" }.into())
                            .cursor_pointer()
                            .child(mono("reset width", 12., p.dim))
                            .hover(|d| d.text_color(p.ink))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let mut pn = this.panels;
                                if rail {
                                    pn.rail_width = None;
                                } else {
                                    pn.right_width = None;
                                }
                                this.set_panels_pub(pn, cx);
                            })),
                    );
                }
                let what = if rail { Pick::Rail } else { Pick::Right };
                control = control.child(self.choice(
                    id,
                    what,
                    vec!["shown".into(), "folded".into()],
                    Some(if folded { "folded" } else { "shown" }),
                    None,
                    p,
                    cx,
                ));
                (control.into_any_element(), Vec::new())
            });
        }
        self.app_row(
            &mut rows,
            "app-label",
            "Daemon label",
            "daemon_label",
            "Names a remote daemon in headers, since its Welcome carries no host name.",
            || {
                let control = self.field(
                    Edit::DaemonLabel,
                    self.app_settings.daemon_label.clone().unwrap_or_else(|| "none".into()).into(),
                    "",
                    false,
                    p,
                    cx,
                );
                (control, Vec::new())
            },
        );
        self.app_row(&mut rows, "app-dash", "Dash URL", "dash_url", "A remote daemon's synaps-dash, through a forwarded port on 127.0.0.1 (other hosts are refused). Empty: a local daemon's own URL file, --dash, or $SYNAPS_DASH_URL. A token pasted with it is kept for this run only, never saved. Never writes config.", || {
            let control = self.field(
                Edit::DashUrl,
                self.app_settings.dash_url.as_deref().map(crate::dash::strip_token).unwrap_or_else(|| "discover".into()).into(),
                "",
                false,
                p,
                cx,
            );
            (control, Vec::new())
        });
        let mut hosts: Vec<SharedString> = vec!["all".into()];
        // A host is offered only up to the writer's bound for it.
        hosts.extend(self.usage.chips.iter().filter(|h| h.len() <= HOST_MAX).map(|h| SharedString::from(h.clone())));
        let host = self.app_settings.usage_host.clone().unwrap_or_else(|| "all".into());
        self.app_row(&mut rows, "app-usage", "Usage opens on", "usage_range", "The range and host Usage shows when it opens.", || {
            let control = v_flex()
                .gap(px(8.))
                .items_end()
                .child(self.choice(
                    "usage-range",
                    Pick::Range,
                    Range::ALL.iter().map(|r| SharedString::from(r.label())).collect(),
                    Some(self.app_settings.usage_range.label()),
                    None,
                    p,
                    cx,
                ))
                .child(self.choice("usage-host", Pick::Host, hosts, Some(&host), None, p, cx))
                .into_any_element();
            (control, Vec::new())
        });
        rows.sort_by_key(|r| r.found.map_or(0, |f| f.rank));
        let rows: Vec<AnyElement> = rows
            .into_iter()
            .enumerate()
            .map(|(i, mut r)| {
                if let Some(n) = self.not_saved(r.key, p, cx) {
                    r.under.push(n);
                }
                if let Some(k) = AppKey::of(r.key) {
                    let at = self.app_saved_at(k, cx);
                    r.control = Self::saved_beside(r.control, r.key, at, p);
                }
                self.render_row(r, i == 0, p, cx)
            })
            .collect();
        (format!("saved on this machine · {path}").into(), rows)
    }

    /// The session rows' current value, from its `RuntimeView`.
    fn session_value(&self, key: &str) -> String {
        let Some(v) = self.model.lives[self.panes.cur.live].stats.view.as_ref() else { return crate::usage::NONE.into() };
        let n = |x: Option<u64>, unit: &str| {
            x.map(|x| if unit.is_empty() { x.to_string() } else { format!("{x} {unit}") }).unwrap_or_else(|| crate::usage::NONE.into())
        };
        let s = |x: &str| if x.is_empty() { crate::usage::NONE.to_string() } else { x.to_string() };
        match key {
            "model" => s(&v.model),
            "thinking" => s(&v.effort),
            "context_window" => ctx_window(v.context_window),
            "compaction_model" => s(&v.compaction_model),
            "api_retries" => n(v.api_retries, ""),
            "subagent_timeout" => n(v.subagent_timeout, "s"),
            "max_tool_output" => n(v.max_tool_output, "bytes"),
            "bash_timeout" => n(v.bash_timeout, "s"),
            "bash_max_timeout" => n(v.bash_max_timeout, "s"),
            _ => crate::usage::NONE.into(),
        }
    }

    /// The session's keep-warm mode, as its last `Status` said
    /// (`keepwarm.mode`, the effective one; [`crate::warm`]): `—` until it
    /// answers. Only a daemon with `cache_expiry` (0.13.0+) is asked.
    fn session_keepwarm(&self) -> String {
        let warm = self.model.lives[self.panes.cur.live].stats.warm.as_ref();
        warm.map(|w| w.mode.clone()).filter(|m| !m.is_empty()).unwrap_or_else(|| crate::usage::NONE.into())
    }

    fn group_session(&self, p: &Palette, cx: &ViewCx, writable: bool) -> (SharedString, Vec<AnyElement>) {
        let target = self.model.tabs.current().filter(|_| self.model.lives[self.panes.cur.live].active.is_some());
        let Some(tab) = target else {
            let rows = if self.take(&Fields::new("open a session to change these", "", "")).is_some() {
                vec![sans("Open a session to see these.", 13., 20., p.dim).py(px(12.)).into_any_element()]
            } else {
                Vec::new()
            };
            return ("no session open".into(), rows);
        };
        let title = [
            self.model.lives[self.panes.cur.live].transcript.title(),
            self.model.active_name(self.panes.cur.live),
            Some(tab.label.as_str()),
        ]
        .into_iter()
        .flatten()
        .find(|t| !t.trim().is_empty())
        .unwrap_or("New session")
        .to_string();
        let line = format!(
            "“{}” · live values · its model and thinking change from the composer, this session only; the rest is read-only here",
            crate::app::one_line(&title, 60)
        );
        let mut rows = Vec::new();
        for key in settings::SESSION_KEYS {
            let Some(d) = settings::def(key) else { continue };
            let Some(found) = self.take(&Fields::new(d.label, key, d.help)) else { continue };
            let mut under = Vec::new();
            if self.shown_config().is_some_and(|c| c.exposes(key)) {
                under.push(
                    h_flex()
                        .mt(px(8.))
                        .gap(px(12.))
                        .items_center()
                        .child(mono("default for new sessions", 12., p.dim))
                        .child(div().flex_1())
                        .child(self.config_control(key, writable, p, cx))
                        .into_any_element(),
                );
                if let Some(t) = self.applies(key) {
                    under.push(mono(t, 12., p.dim).debug_selector(move || format!("tag {key} = {t}")).into_any_element());
                }
                under.extend(self.config_under(key, writable, p));
            }
            rows.push(Row {
                id: format!("session-{key}").into(),
                label: d.label.into(),
                key,
                help: d.help,
                control: Self::session_locked(key, self.session_value(key), p),
                tag: None,
                under,
                found: Some(found),
            });
        }
        if let Some(found) = self.take(&Fields::new("System prompt", "system_prompt", SYSTEM_PROMPT_HELP)) {
            let prompt = self.model.lives[self.panes.cur.live].stats.view.as_ref().and_then(|v| v.system_prompt.clone());
            let well = match prompt {
                Some(text) => {
                    // Its selector says the body it was handed (tests).
                    let body = SharedString::from(text);
                    let said = body.clone();
                    v_flex()
                        .child(
                            div()
                                .id("system-prompt")
                                .debug_selector(move || format!("system-prompt = {said}"))
                                .mt(px(8.))
                                .max_h(px(12. * 20.))
                                .overflow_y_scroll()
                                .track_scroll(&self.bars.prompt.scroll)
                                .p(px(10.))
                                .bg(p.code)
                                .font_family(crate::fonts::MONO)
                                .text_px(13.)
                                .line_height(px(20.))
                                .text_color(p.code_fg)
                                .whitespace_normal()
                                .child(body),
                        )
                        .child(self.bars.prompt.bar(crate::scrollbar::Ground::Canvas))
                        .into_any_element()
                }
                None => mono("the daemon's default prompt", 13., p.dim)
                    .mt(px(4.))
                    .debug_selector(|| "system-prompt-none".into())
                    .into_any_element(),
            };
            rows.push(Row {
                id: "session-system".into(),
                label: "System prompt".into(),
                key: "system_prompt",
                help: SYSTEM_PROMPT_HELP,
                control: Self::locked(
                    "system_prompt",
                    if self.model.lives[self.panes.cur.live].stats.view.as_ref().is_some_and(|v| v.system_prompt.is_some()) {
                        "custom"
                    } else {
                        "default"
                    },
                    p,
                ),
                tag: None,
                under: vec![well],
                found: Some(found),
            });
        }
        // Found by its own words, else (unlit) by the config key's it shows.
        // Both are taken, whatever is found: a walk takes the same rows.
        let own = self.take(&Fields::new("Cache keep-warm", "keepwarm", KEEPWARM_HELP));
        let by_key = settings::def("cache_keepwarm")
            .and_then(|d| self.settings.search.borrow_mut().take_unlit(&Fields::new(d.label, d.key, d.help)));
        let keepwarm = own.or(by_key);
        if let Some(found) = keepwarm {
            rows.push(Row {
                id: "session-keepwarm".into(),
                label: "Cache keep-warm".into(),
                key: "keepwarm",
                help: KEEPWARM_HELP,
                control: Self::session_locked("keepwarm", self.session_keepwarm(), p),
                tag: None,
                under: Vec::new(),
                found: Some(found),
            });
        }
        rows.sort_by_key(|r| r.found.map_or(0, |f| f.rank));
        let rows = rows.into_iter().enumerate().map(|(i, r)| self.render_row(r, i == 0, p, cx)).collect();
        (line.into(), rows)
    }

    /// DAEMON CONFIG; its header says where the values come from and, when
    /// they cannot be written, why (the gate's first unproven link).
    fn group_config(&self, p: &Palette, cx: &ViewCx, gate: &Result<(), String>) -> (SharedString, Vec<AnyElement>) {
        // The file the dash wrote, else the one read here.
        let path = self
            .shown_config()
            .map(|c| c.path.clone())
            .filter(|s| !s.is_empty())
            .or_else(|| self.settings.reference.as_ref().map(|r| r.path.clone()))
            .unwrap_or_else(|| "the local config".into());
        let local = self.dash.is_local();
        let foreign = self.config_foreign();
        let line = match (gate, &self.dash.answer, self.shown_config()) {
            (Ok(()), _, _) => format!("{path} · saved through synaps-dash"),
            (Err(why), Answer::Up, Some(_)) => format!("{path} · CONFIG read-only: {why}"),
            (_, Answer::Down(Fail::Token), _) => "synaps-dash token changed: reading its URL file again · CONFIG read-only".into(),
            (_, Answer::Down(e), _) if !local => format!("{} · values unknown", self.fail_text(e)),
            // N-M1: the dash answers for another daemon (avante's dash read
            // for a forwarded jade): its values are not this daemon's.
            (_, _, None) if !local && foreign.is_some() => format!("values unknown · {}", foreign.unwrap_or_default()),
            (Err(why), _, _) if local => format!("read from {path} · CONFIG read-only: {why}"),
            (Err(why), _, _) => format!("values unknown · CONFIG read-only: {why}"),
        };
        // The session rows' defaults show under them while a session is
        // open and the dash has them; else here.
        let session_open = self.model.tabs.current().is_some() && self.model.lives[self.panes.cur.live].active.is_some();
        let under_session = |key: &str| session_open && self.shown_config().is_some_and(|c| c.exposes(key));
        let writable = gate.is_ok();
        let mut out = Vec::new();
        for (cat, keys) in settings::CONFIG_GROUPS {
            let mut rows = Vec::new();
            for &key in keys {
                if settings::SESSION_KEYS.contains(&key) && under_session(key) {
                    continue;
                }
                let Some(d) = settings::def(key) else { continue };
                let Some(found) = self.take(&Fields::new(d.label, key, d.help)) else { continue };
                rows.push(Row {
                    id: format!("config-{key}").into(),
                    label: d.label.into(),
                    key,
                    help: d.help,
                    control: self.config_control(key, writable, p, cx),
                    tag: self.applies(key),
                    under: self.config_under(key, writable, p),
                    found: Some(found),
                });
            }
            rows.sort_by_key(|r| r.found.map_or(0, |f| f.rank));
            if !rows.is_empty() {
                out.push(Self::sub_head(cat.title(), p));
                out.extend(rows.into_iter().enumerate().map(|(i, r)| self.render_row(r, i == 0, p, cx)));
            }
        }
        (line.into(), out)
    }

    fn group_more(&self, p: &Palette) -> (SharedString, Vec<AnyElement>) {
        let mut out = Vec::new();
        for cat in Cat::ORDER {
            let mut found: Vec<(Found, &settings::Def)> =
                settings::reference(cat).filter_map(|d| self.take(&Fields::new(d.label, d.key, d.help)).map(|f| (f, d))).collect();
            found.sort_by_key(|(f, _)| f.rank);
            let search = self.settings.search.borrow();
            let mut lines = Vec::new();
            for (f, d) in found {
                let value = self.reference_shown(d.key);
                let first = lines.is_empty();
                let hits = search.hits_of(f.row);
                let (lr, kr) = (Runs::of(hits, find::LABEL, 0, d.label.len(), 0), Runs::of(hits, find::KEY, 0, d.key.len(), 0));
                lines.push(
                    h_flex()
                        .debug_selector(|| format!("reference {} = {value}", d.key))
                        .w_full()
                        .min_h(px(32.))
                        .gap(px(12.))
                        .items_center()
                        .when(!first, |d| d.border_t_1().border_color(p.hair))
                        .child(lit(sans_in(14., 22., p.ink), d.label.into(), lr, p.you, Some((d.key.into(), "label"))).flex_none())
                        .child(
                            lit(mono_in(12., p.dim), d.key.into(), kr, p.you, Some((d.key.into(), "key")))
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis(),
                        )
                        .child(mono(value, 13., p.dim).flex_none().max_w(px(220.)).overflow_hidden().text_ellipsis())
                        .child(crate::glyph::session_mark(State::Locked, crate::glyph::Ground::Canvas, p))
                        // Its line has a selector of its own: the hit mark is
                        // a child out of the flow (no room, no gap).
                        .when(!lr.is_empty() || !kr.is_empty(), |l| {
                            let key = d.key;
                            l.child(div().absolute().debug_selector(move || format!("hit-{key}")))
                        })
                        .into_any_element(),
                );
            }
            if !lines.is_empty() {
                out.push(Self::sub_head(cat.title(), p));
                out.extend(lines);
            }
        }
        let n: usize = Cat::ORDER.iter().map(|&c| settings::reference(c).count()).sum();
        let source = match &self.settings.reference {
            Some(settings::Reference { path, values: Ok(_) }) => format!("values from {path}"),
            Some(settings::Reference { path, values: Err(e) }) => format!("{path} could not be read ({e}): —"),
            None if self.dash.is_local() => "reading the local config".to_string(),
            None => "a remote daemon's values: —".to_string(),
        };
        (format!("{n} keys · edit with /settings in the TUI on the daemon's machine · {source}").into(), out)
    }

    fn group_providers(&self, p: &Palette) -> (SharedString, Vec<AnyElement>) {
        let on = !self.settings.search.borrow().query().is_empty();
        let rows: Vec<AnyElement> = match self.shown_config() {
            Some(c) if !c.providers.is_empty() => {
                let mut found: Vec<(Found, String, String)> = c
                    .providers
                    .iter()
                    .filter_map(|v| {
                        let mut s = format!("{} · from {}", v.kind, v.source);
                        if let Some(n) = &v.note {
                            s.push_str(&format!(" · {n}"));
                        }
                        self.take_made(&Fields::fact(&v.name, &s)).map(|f| (f, v.name.clone(), s))
                    })
                    .collect();
                found.sort_by_key(|(f, ..)| f.rank);
                let search = self.settings.search.borrow();
                found
                    .into_iter()
                    .enumerate()
                    .map(|(i, (f, name, s))| Self::info_row_owned(name, s, i == 0, search.hits_of(f.row), p))
                    .collect()
            }
            Some(_) if !on => vec![sans("No providers configured.", 13., 20., p.dim).py(px(8.)).into_any_element()],
            None if !on => vec![sans(crate::usage::NONE, 13., 20., p.dim).py(px(8.)).into_any_element()],
            _ => Vec::new(),
        };
        ("read-only · keys and installs are managed in the TUI · never a key".into(), rows)
    }

    fn group_plugins(&self, p: &Palette) -> (SharedString, Vec<AnyElement>) {
        let on = !self.settings.search.borrow().query().is_empty();
        let rows: Vec<AnyElement> = match self.shown_config() {
            Some(c) if !c.plugins.is_empty() => {
                // Its description isn't drawn: found by it, never lit in it.
                let mut found: Vec<(Found, String, String)> = c
                    .plugins
                    .iter()
                    .filter_map(|v| {
                        let s = format!(
                            "{}{} · {}",
                            if v.enabled { "enabled" } else { "disabled" },
                            v.version.as_deref().map(|x| format!(" · v{x}")).unwrap_or_default(),
                            v.scope
                        );
                        let f = Fields { help: &v.description, ..Fields::fact(&v.name, &s) };
                        self.take_made(&f).map(|f| (f, v.name.clone(), s))
                    })
                    .collect();
                found.sort_by_key(|(f, ..)| f.rank);
                let search = self.settings.search.borrow();
                found
                    .into_iter()
                    .enumerate()
                    .map(|(i, (f, name, s))| Self::info_row_owned(name, s, i == 0, search.hits_of(f.row), p))
                    .collect()
            }
            Some(_) if !on => vec![sans("No plugins installed.", 13., 20., p.dim).py(px(8.)).into_any_element()],
            None if !on => vec![sans(crate::usage::NONE, 13., 20., p.dim).py(px(8.)).into_any_element()],
            _ => Vec::new(),
        };
        ("read-only · keys and installs are managed in the TUI".into(), rows)
    }

    fn info_row_owned(label: String, value: String, first: bool, hits: &[Hit], p: &Palette) -> AnyElement {
        let (lr, vr) = (Runs::of(hits, find::LABEL, 0, label.len(), 0), Runs::of(hits, find::VALUE, 0, value.len(), 0));
        let name = SharedString::from(label.clone());
        h_flex()
            .debug_selector(|| format!("fact {label} = {value}"))
            .w_full()
            .min_h(px(36.))
            .py(px(8.))
            .gap(px(24.))
            .items_center()
            .when(!first, |d| d.border_t_1().border_color(p.hair))
            .child(
                lit(sans_in(14., 22., p.ink), name.clone(), lr, p.you, Some((name.clone(), "label")))
                    .w(px(180.))
                    .flex_none()
                    .overflow_hidden()
                    .text_ellipsis(),
            )
            .child(lit(mono_in(13., p.dim), value.into(), vr, p.you, Some((name, "value"))).flex_1().min_w_0().whitespace_normal())
            .into_any_element()
    }
}
