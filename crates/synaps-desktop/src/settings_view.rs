//! The Settings view: one scrolling column of groups, each saying where it
//! writes (`usage-design/spec.md` section 2). APP rows save to this
//! machine's `settings.json`; CONFIG rows go through synaps-dash's `POST
//! /api/config` when the daemon is local; everything else is shown
//! read-only with its value and the reason, never as a disabled control.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use gpui_kit::component::input::Input;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::app::{AppView, display, mono};
use crate::chrome::glyph;
use crate::dash::Fail;
use crate::rail::State;
use crate::settings::{self, Cat};
use crate::theme::{self, Palette};
use crate::usage::Range;
use crate::views::Answer;

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
    /// A remote daemon's dash; kept here with its token for this run, saved
    /// without it.
    pub dash_url: Option<String>,
    pub usage_range: Range,
    pub usage_host: Option<String>,
}

impl AppSettings {
    pub fn load(path: Option<&Path>) -> Self {
        let get = |k: &str| path.and_then(|p| theme::load_str(p, k));
        AppSettings {
            daemon_label: get("daemon_label"),
            dash_url: get("dash_url"),
            usage_range: get("usage_range").and_then(|r| Range::parse(&r)).unwrap_or_default(),
            usage_host: get("usage_host").filter(|h| h != "all"),
        }
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
    Rail,
    Right,
    Range,
    Host,
}

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

#[derive(Default)]
pub(crate) struct SettingsUi {
    /// The find text, lowercase.
    pub find: String,
    pub scroll: ScrollHandle,
    pub editing: Option<Edit>,
    /// The row whose whole help shows (by key).
    pub open_help: Option<&'static str>,
    /// The open dropdown (by its control id).
    pub dropdown: Option<&'static str>,
    pub more_open: bool,
    pub writes: HashMap<&'static str, Write>,
    /// A local daemon's config as `key = value` (REFERENCE values), read
    /// when Settings opens.
    pub reference: Option<Arc<HashMap<String, String>>>,
    /// The outcome of a THIS APP change, under its row (by key).
    pub app_note: Option<(&'static str, String)>,
}

/// One row's parts.
struct Row {
    id: SharedString,
    label: SharedString,
    key: &'static str,
    help: &'static str,
    control: AnyElement,
    tag: Option<&'static str>,
    under: Vec<AnyElement>,
}

fn sans(text: impl Into<SharedString>, size: f32, line: f32, color: Hsla) -> Div {
    div().text_size(px(size)).line_height(px(line)).text_color(color).child(text.into())
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
            Pick::Rail | Pick::Right => {
                let rail = what == Pick::Rail;
                let folded = if rail { self.panels.rail_collapsed } else { self.panels.right_collapsed };
                if (value == "folded") != folded {
                    self.toggle_panel(rail, cx);
                }
            }
            Pick::Range => {
                if let Some(r) = Range::parse(value) {
                    self.app_settings.usage_range = r;
                    self.save_app("usage_range", Some(r.key().to_string()));
                }
            }
            Pick::Host => {
                let h = (value != "all").then(|| value.to_string());
                self.app_settings.usage_host = h.clone();
                self.save_app("usage_host", h);
            }
        }
        cx.notify();
    }

    /// A THIS APP key, written off the UI thread.
    fn save_app(&mut self, key: &'static str, value: Option<String>) {
        let Some(path) = theme::settings_path() else {
            self.settings.app_note = Some((key, "not saved: no config folder".into()));
            return;
        };
        self.settings.app_note = None;
        std::thread::spawn(move || {
            if let Err(e) = theme::save_settings(&path, &[(key, value.into())]) {
                eprintln!("synaps-desktop: {key} not saved: {e}");
            }
        });
    }

    /// Start editing a field: the shared input takes its value and focus.
    pub(crate) fn settings_edit(&mut self, e: Edit, window: &mut Window, cx: &mut Context<Self>) {
        let value = match e {
            Edit::Config(k) => self.dash.config.as_ref().and_then(|c| c.value(k)).unwrap_or("").to_string(),
            Edit::DaemonLabel => self.app_settings.daemon_label.clone().unwrap_or_default(),
            Edit::DashUrl => self.app_settings.dash_url.as_deref().map(crate::dash::strip_token).unwrap_or_default(),
        };
        self.settings.editing = Some(e);
        self.settings_input.update(cx, |s, cx| {
            s.set_value(value, window, cx);
            s.focus(window, cx);
        });
        cx.notify();
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
                self.save_app("daemon_label", v);
            }
            Edit::DashUrl => {
                let saved = (!typed.is_empty()).then(|| crate::dash::strip_token(&typed));
                self.app_settings.dash_url = (!typed.is_empty()).then_some(typed);
                self.save_app("dash_url", saved);
                self.dash_refind();
                self.ask_config(cx);
            }
        }
        cx.notify();
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

    /// The find field changed.
    pub(crate) fn settings_find(&mut self, text: String, cx: &mut Context<Self>) {
        let f = text.trim().to_lowercase();
        if f != self.settings.find {
            self.settings.find = f;
            cx.notify();
        }
    }

    /// A local daemon's config as `key = value`, read once per opening.
    fn reference_values(&mut self) {
        if self.dash.local != Some(true) || self.settings.reference.is_some() {
            return;
        }
        let base = synaps_client::discovery::base_dir();
        let profile = self.model.daemon.as_ref().and_then(|d| d.profile.clone());
        let path = theme::read_path(&base, profile.as_deref(), "config");
        let text = std::fs::read_to_string(path).unwrap_or_default();
        self.settings.reference = Some(Arc::new(settings::read_config(&text)));
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
        cx: &Context<Self>,
    ) -> AnyElement {
        let current = sending.or(selected);
        if opts.len() <= 4 {
            let mut seg = h_flex().h(px(28.)).border_1().border_color(p.edge);
            for (i, o) in opts.into_iter().enumerate() {
                let on = current == Some(o.as_ref());
                let color = match (on, sending.is_some()) {
                    (true, true) => p.dim,
                    (true, false) => p.ink,
                    _ => p.dim,
                };
                let v = o.clone();
                seg = seg.child(
                    div()
                        .id((id, i))
                        .h_full()
                        .px(px(12.))
                        .flex()
                        .items_center()
                        .when(i > 0, |d| d.border_l_1().border_color(p.edge))
                        .when(on, |d| d.bg(p.raised).border_b_2().border_color(p.focus))
                        .cursor_pointer()
                        .hover(|d| d.bg(p.raised))
                        .child(mono(o, 12.5, color))
                        .on_click(cx.listener(move |this, _, _, cx| this.settings_pick(what, &v, cx))),
                );
            }
            return seg.into_any_element();
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
                        .h(px(26.))
                        .px(px(10.))
                        .items_center()
                        .cursor_pointer()
                        .when(on, |d| d.bg(p.raised))
                        .hover(|d| d.bg(p.raised))
                        .child(mono(o, 12.5, if on { p.ink } else { p.prose }))
                        .on_click(cx.listener(move |this, _, _, cx| this.settings_pick(what, &v, cx))),
                );
            }
        }
        v_flex()
            .items_end()
            .child(
                h_flex()
                    .id(id)
                    .w(px(CONTROL))
                    .h(px(28.))
                    .px(px(10.))
                    .items_center()
                    .border_1()
                    .border_color(if open { p.focus } else { p.edge })
                    .cursor_pointer()
                    .child(mono(shown, 12.5, if sending.is_some() { p.dim } else { p.prose }).flex_1())
                    .child(mono(if open { "↑" } else { "↓" }, 12., p.dim))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.settings.dropdown = if this.settings.dropdown == Some(id) { None } else { Some(id) };
                        cx.notify();
                    })),
            )
            .when(open, |d| d.child(list))
            .into_any_element()
    }

    /// A number or text field: its value in a box; click to edit (Enter
    /// saves, Esc leaves it).
    fn field(&self, e: Edit, value: SharedString, unit: &'static str, sending: bool, p: &Palette, cx: &Context<Self>) -> AnyElement {
        let editing = self.settings.editing == Some(e);
        let id: SharedString = match e {
            Edit::Config(k) => format!("field-{k}").into(),
            Edit::DaemonLabel => "field-daemon-label".into(),
            Edit::DashUrl => "field-dash-url".into(),
        };
        let body = if editing {
            div()
                .flex_1()
                .min_w_0()
                .font_family(crate::fonts::MONO)
                .text_size(px(12.5))
                .child(Input::new(&self.settings_input).appearance(false))
                .into_any_element()
        } else {
            mono(value, 12.5, if sending { p.dim } else { p.prose }).flex_1().min_w_0().overflow_hidden().text_ellipsis().into_any_element()
        };
        h_flex()
            .id(id)
            .w(px(CONTROL))
            .h(px(28.))
            .px(px(10.))
            .gap(px(8.))
            .items_center()
            .border_1()
            .border_color(if editing { p.focus } else { p.edge })
            .when(!editing, |d| d.cursor_text())
            .child(body)
            .when(!unit.is_empty(), |d| d.child(mono(unit, 12., p.dim)))
            .on_click(cx.listener(move |this, _, window, cx| {
                if this.settings.editing != Some(e) {
                    this.settings_edit(e, window, cx);
                }
            }))
            .into_any_element()
    }

    /// A read-only value: dim text, no border, then the rail's LOCKED glyph.
    fn locked(value: impl Into<SharedString>, p: &Palette) -> AnyElement {
        h_flex()
            .gap(px(10.))
            .items_center()
            .justify_end()
            .max_w(px(CONTROL + 120.))
            .child(mono(value, 12.5, p.dim).overflow_hidden().text_ellipsis())
            .child(glyph(State::Locked, p))
            .into_any_element()
    }

    /// The control for a CONFIG key: editable through the dash, else its
    /// value read-only (`—` when it cannot be read).
    fn config_control(&self, key: &'static str, p: &Palette, cx: &Context<Self>) -> AnyElement {
        let snap = self.dash.config.as_ref().filter(|c| c.exposes(key));
        let raw = snap.and_then(|c| c.value(key));
        let sending = match self.settings.writes.get(key) {
            Some(Write::Sending(v)) => Some(v.as_str()),
            _ => None,
        };
        if snap.is_none() {
            let value = match self.settings.reference.as_ref() {
                Some(r) => settings::shown(key, r.get(key).map(String::as_str)),
                None => crate::usage::NONE.into(),
            };
            return Self::locked(value, p);
        }
        let shown = settings::shown(key, raw);
        if !self.dash.config_writable() {
            return Self::locked(shown, p);
        }
        let id: &'static str = settings::def(key).map_or("config", |d| d.key);
        match settings::config_choices(key) {
            Some(opts) => {
                self.choice(id, Pick::Config(key), opts.iter().map(|o| SharedString::from(*o)).collect(), Some(&shown), sending, p, cx)
            }
            None => {
                self.field(Edit::Config(key), sending.unwrap_or(&shown).to_string().into(), settings::unit(key), sending.is_some(), p, cx)
            }
        }
    }

    /// Lines under a CONFIG row: the refusal, or that it waits for a reload.
    fn config_under(&self, key: &'static str, p: &Palette) -> Vec<AnyElement> {
        let mut out = Vec::new();
        if let Some(Write::Refused(m)) = self.settings.writes.get(key) {
            out.push(mono(m.clone(), 12., p.err).whitespace_normal().into_any_element());
        }
        if let Some(c) = self.dash.config.as_ref().filter(|c| c.pending.iter().any(|k| k == key)) {
            out.push(mono(format!("saved · applies after: {}", c.reload_cmd), 12., p.warn).whitespace_normal().into_any_element());
        }
        out
    }

    fn applies(&self, key: &str) -> Option<&'static str> {
        self.dash.config.as_ref().and_then(|c| c.applies.get(key)).and_then(|a| settings::applies_tag(a))
    }

    // ── layout ─────────────────────────────────────────────────────────

    fn render_row(&self, r: Row, first: bool, p: &Palette, cx: &Context<Self>) -> AnyElement {
        let open = self.settings.open_help == Some(r.key);
        let help: SharedString = if open { r.help.into() } else { settings::first_sentence(r.help).into() };
        let key = r.key;
        let more = r.help.len() > settings::first_sentence(r.help).len();
        let line = h_flex()
            .w_full()
            .gap(px(24.))
            .items_start()
            .child(
                v_flex()
                    .id("help")
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .when(more, |d| d.cursor_pointer())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.settings.open_help = if this.settings.open_help == Some(key) { None } else { Some(key) };
                        cx.notify();
                    }))
                    .child(
                        h_flex()
                            .gap(px(10.))
                            .items_baseline()
                            .child(sans(r.label, 14., 22., p.ink))
                            .when(!key.is_empty(), |d| d.child(mono(key, 12., p.dim))),
                    )
                    .when(!r.help.is_empty(), |d| d.child(sans(help, 13., 20., p.dim).whitespace_normal()))
                    .when_some(r.tag, |d, t| d.child(mono(t, 12., p.dim).mt(px(2.)))),
            )
            .child(div().flex_none().flex().justify_end().child(r.control));
        // Under it, full width: a session row's default, a refusal, a reload.
        v_flex()
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

    fn group_head(&self, ix: usize, title: SharedString, line: SharedString, p: &Palette, cx: &Context<Self>) -> Div {
        let more = ix == 4;
        let open = self.settings.more_open || !self.settings.find.is_empty();
        v_flex()
            .pt(px(40.))
            .pb(px(8.))
            .gap(px(4.))
            .child(
                h_flex()
                    .id(("group-head", ix))
                    .gap(px(10.))
                    .items_baseline()
                    .when(more, |d| {
                        d.cursor_pointer().on_click(cx.listener(|this, _, _, cx| {
                            this.settings.more_open = !this.settings.more_open;
                            cx.notify();
                        }))
                    })
                    .when(more, |d| d.child(mono(if open { "−" } else { "+" }, 13., p.dim)))
                    .child(display(title, 13., p.header)),
            )
            .child(mono(line, 12., p.dim).whitespace_normal())
    }

    fn sub_head(title: &'static str, p: &Palette) -> AnyElement {
        div().pt(px(16.)).pb(px(4.)).child(display(title, 12., p.dim)).into_any_element()
    }

    /// The Settings view, `pane` px wide.
    pub(crate) fn render_settings(&mut self, pane: f32, p: &Palette, cx: &Context<Self>) -> AnyElement {
        self.reference_values();
        let col = (pane - 48.).clamp(320., COLUMN);
        let index = pane >= INDEX_FROM;
        let left = if index { ((pane - col) / 2.).max(INDEX + 48.) } else { ((pane - col) / 2.).max(24.) };
        let find = self.settings.find.clone();
        let wrap = |inner: AnyElement| h_flex().w_full().pl(px(left)).child(div().w(px(col)).flex_none().child(inner)).into_any_element();
        let mut blocks: Vec<AnyElement> = Vec::with_capacity(8);
        // Title and find.
        blocks.push(wrap(
            h_flex()
                .pt(px(28.))
                .gap(px(24.))
                .items_center()
                .child(display("S E T T I N G S", 16., p.header))
                .child(div().flex_1())
                .child(
                    h_flex().w(px(CONTROL)).h(px(28.)).px(px(10.)).items_center().border_1().border_color(p.edge).child(
                        div()
                            .flex_1()
                            .font_family(crate::fonts::MONO)
                            .text_size(px(12.5))
                            .child(Input::new(&self.settings_find).appearance(false)),
                    ),
                )
                .into_any_element(),
        ));
        let groups = [
            self.group_connection(p, &find),
            self.group_app(p, cx, &find),
            self.group_session(p, cx, &find),
            self.group_config(p, cx, &find),
            self.group_more(p, &find),
            self.group_providers(p, &find),
            self.group_plugins(p, &find),
        ];
        let mut shown_groups = Vec::new();
        for (ix, (line, rows)) in groups.into_iter().enumerate() {
            if !find.is_empty() && rows.is_empty() {
                continue;
            }
            shown_groups.push(ix);
            let mut g = self.group_head(ix, GROUPS[ix].into(), line, p, cx);
            let folded = ix == 4 && !self.settings.more_open && find.is_empty();
            if !folded {
                g = g.children(rows);
            }
            blocks.push(wrap(g.into_any_element()));
        }
        if !find.is_empty() && shown_groups.is_empty() {
            blocks.push(wrap(sans(format!("Nothing matches “{find}”."), 13., 20., p.dim).pt(px(40.)).into_any_element()));
        }
        blocks.push(div().h(px(64.)).into_any_element());
        // The group at the top of the view (child 0 is the title).
        let top = self.settings.scroll.top_item().saturating_sub(1);
        let in_view = shown_groups.get(top.min(shown_groups.len().saturating_sub(1))).copied();
        let column = v_flex()
            .id("settings-scroll")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.settings.scroll)
            .on_scroll(cx.listener(|_, _, _, cx| cx.notify()))
            .children(blocks);
        let mut root = div().relative().size_full().bg(p.canvas).child(column);
        if index {
            let mut list = v_flex().absolute().top(px(84.)).left(px(left - INDEX - 24.)).w(px(INDEX)).gap(px(2.));
            for (pos, &ix) in shown_groups.iter().enumerate() {
                let on = in_view == Some(ix);
                list = list.child(
                    div()
                        .id(("settings-index", ix))
                        .py(px(5.))
                        .cursor_pointer()
                        .child(display(GROUPS[ix], 12., if on { p.ink } else { p.dim }))
                        .hover(|d| d.text_color(p.ink))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.settings.scroll.scroll_to_top_of_item(pos + 1);
                            cx.notify();
                        })),
                );
            }
            root = root.child(list);
        }
        root.into_any_element()
    }

    // ── the groups: (header line, rows) ────────────────────────────────

    fn info_row(label: &'static str, value: impl Into<SharedString>, first: bool, p: &Palette) -> AnyElement {
        h_flex()
            .w_full()
            .min_h(px(36.))
            .py(px(8.))
            .gap(px(24.))
            .items_center()
            .when(!first, |d| d.border_t_1().border_color(p.hair))
            .child(sans(label, 14., 22., p.ink).w(px(180.)).flex_none())
            .child(mono(value, 12.5, p.prose).flex_1().min_w_0().whitespace_normal())
            .into_any_element()
    }

    fn group_connection(&self, p: &Palette, find: &str) -> (SharedString, Vec<AnyElement>) {
        let d = self.model.daemon.as_ref();
        let socket = self.dash.launch.resolved.as_ref().map(|s| s.display().to_string()).unwrap_or_else(|| crate::usage::NONE.into());
        let kind = match (self.dash.local, d) {
            (_, None) if self.synthetic => "offline (synthetic sessions)".to_string(),
            (_, None) => "not connected".to_string(),
            (Some(true), _) => "local daemon".to_string(),
            _ => match &self.app_settings.daemon_label {
                Some(l) => format!("{l} · remote daemon"),
                None => "remote daemon".to_string(),
            },
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
                    Answer::Down(e) => e.to_string(),
                };
                format!("{} · {state} · from {}", t.shown(), src.describe())
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
            ("Version", d.map(|d| d.version.clone()).unwrap_or_else(|| crate::usage::NONE.into())),
            ("Process", d.map(|d| d.pid.to_string()).unwrap_or_else(|| crate::usage::NONE.into())),
            ("Profile", d.map(|d| d.profile.clone().unwrap_or_else(|| "default".into())).unwrap_or_else(|| crate::usage::NONE.into())),
            ("Protocol", synaps_client::protocol::PROTOCOL_VERSION.to_string()),
            ("Socket", socket),
            ("synaps-dash", dash),
            ("Telemetry sources", sources),
        ];
        let rows = rows
            .into_iter()
            .filter(|(l, v)| settings::matches(find, l, "", v))
            .enumerate()
            .map(|(i, (l, v))| Self::info_row(l, v, i == 0, p))
            .collect();
        (line.into(), rows)
    }

    fn group_app(&self, p: &Palette, cx: &Context<Self>, find: &str) -> (SharedString, Vec<AnyElement>) {
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
            div()
                .id(id)
                .h(px(28.))
                .px(px(12.))
                .flex()
                .items_center()
                .border_1()
                .border_color(p.edge)
                .cursor_pointer()
                .hover(|d| d.bg(p.raised))
                .child(display("C H O O S E", 12., p.prose))
                .on_click(cx.listener(|this, _, window, cx| this.open_picker(window, cx)))
        };
        let theme_value = if self.theme_follows_config {
            format!("{} · follows the Synaps config", scheme.choice.theme)
        } else {
            scheme.choice.theme.clone()
        };
        rows.push(Row {
            id: "app-theme".into(),
            label: "Theme".into(),
            key: "theme",
            help: "One of the Synaps TUI's themes. Following the Synaps config shows the theme its `theme =` names.",
            control: match pinned(self.theme_source, "--theme", theme::ENV_THEME) {
                Some(why) => Self::locked(format!("{} · {why}", scheme.choice.theme), p),
                None => h_flex()
                    .gap(px(12.))
                    .items_center()
                    .child(mono(theme_value, 12.5, p.prose))
                    .child(open_picker("app-theme-pick"))
                    .into_any_element(),
            },
            tag: None,
            under: Vec::new(),
        });
        let accent = scheme.choice.accent.map(theme::to_hex).unwrap_or_else(|| "none (the theme's)".into());
        rows.push(Row {
            id: "app-accent".into(),
            label: "Accent".into(),
            key: "accent",
            help: "Replaces the theme's “you” and focus colours; never a state.",
            control: match pinned(self.accent_source, "--accent", theme::ENV_ACCENT) {
                Some(why) => Self::locked(format!("{accent} · {why}"), p),
                None => h_flex()
                    .gap(px(12.))
                    .items_center()
                    .child(mono(accent, 12.5, p.prose))
                    .child(open_picker("app-accent-pick"))
                    .into_any_element(),
            },
            tag: None,
            under: Vec::new(),
        });
        let size = self.panels.text_size.to_string();
        rows.push(Row {
            id: "app-text".into(),
            label: "Text size".into(),
            key: "text_size",
            help: "The centre pane's text, in px (Ctrl+= and Ctrl+- step it).",
            control: self.choice(
                "text-size",
                Pick::TextSize,
                theme::TEXT_STEPS.iter().map(|s| SharedString::from(s.to_string())).collect(),
                Some(&size),
                None,
                p,
                cx,
            ),
            tag: None,
            under: Vec::new(),
        });
        for (rail, label, key, id) in
            [(true, "Sessions rail", "rail_collapsed", "app-rail"), (false, "Right panel", "right_collapsed", "app-right")]
        {
            let folded = if rail { self.panels.rail_collapsed } else { self.panels.right_collapsed };
            let width = if rail { self.panels.rail_width } else { self.panels.right_width };
            let mut control = h_flex().gap(px(12.)).items_center();
            if width.is_some() {
                control = control.child(
                    div()
                        .id(if rail { "rail-reset" } else { "right-reset" })
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
            rows.push(Row {
                id: id.into(),
                label: label.into(),
                key,
                help: if rail {
                    "Shown or folded to its strip (Ctrl+B); its dragged width."
                } else {
                    "Shown or folded to its strip (Ctrl+Alt+B); its dragged width."
                },
                control: control.into_any_element(),
                tag: None,
                under: Vec::new(),
            });
        }
        rows.push(Row {
            id: "app-label".into(),
            label: "Daemon label".into(),
            key: "daemon_label",
            help: "Names a remote daemon in headers, since its Welcome carries no host name.",
            control: self.field(
                Edit::DaemonLabel,
                self.app_settings.daemon_label.clone().unwrap_or_else(|| "none".into()).into(),
                "",
                false,
                p,
                cx,
            ),
            tag: None,
            under: Vec::new(),
        });
        rows.push(Row {
            id: "app-dash".into(),
            label: "Dash URL".into(),
            key: "dash_url",
            help: "A remote daemon's synaps-dash (a forwarded port). Empty: a local daemon's own URL file, --dash, or $SYNAPS_DASH_URL. Its token is kept for this run only, never saved.",
            control: self.field(
                Edit::DashUrl,
                self.app_settings.dash_url.as_deref().map(crate::dash::strip_token).unwrap_or_else(|| "discover".into()).into(),
                "",
                false,
                p,
                cx,
            ),
            tag: None,
            under: Vec::new(),
        });
        let mut hosts: Vec<SharedString> = vec!["all".into()];
        hosts.extend(self.usage.chips.iter().map(|h| SharedString::from(h.clone())));
        let host = self.app_settings.usage_host.clone().unwrap_or_else(|| "all".into());
        rows.push(Row {
            id: "app-usage".into(),
            label: "Usage opens on".into(),
            key: "usage_range",
            help: "The range and host Usage shows when it opens.",
            control: v_flex()
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
                .into_any_element(),
            tag: None,
            under: Vec::new(),
        });
        let rows: Vec<AnyElement> = rows
            .into_iter()
            .filter(|r| settings::matches(find, &r.label, r.key, r.help))
            .enumerate()
            .map(|(i, mut r)| {
                if let Some((_, n)) = self.settings.app_note.as_ref().filter(|(k, _)| *k == r.key) {
                    r.under.push(mono(n.clone(), 12., p.err).into_any_element());
                }
                self.render_row(r, i == 0, p, cx)
            })
            .collect();
        (format!("saved on this machine · {path}").into(), rows)
    }

    /// The session rows' current value, from its `RuntimeView`.
    fn session_value(&self, key: &str) -> String {
        let Some(v) = self.model.stats.view.as_ref() else { return crate::usage::NONE.into() };
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
            _ => crate::usage::NONE.into(),
        }
    }

    fn group_session(&self, p: &Palette, cx: &Context<Self>, find: &str) -> (SharedString, Vec<AnyElement>) {
        let target = self.model.tabs.current().filter(|_| self.model.active.is_some());
        let Some(tab) = target else {
            let rows = if settings::matches(find, "open a session to change these", "", "") {
                vec![sans("Open a session to see these.", 13., 20., p.dim).py(px(12.)).into_any_element()]
            } else {
                Vec::new()
            };
            return ("no session open".into(), rows);
        };
        let title = [self.model.transcript.title(), self.model.active_name(), Some(tab.label.as_str())]
            .into_iter()
            .flatten()
            .find(|t| !t.trim().is_empty())
            .unwrap_or("New session")
            .to_string();
        let line =
            format!("“{}” · live values · read-only here until synaps-client can send Set (PR #9)", crate::app::one_line(&title, 60));
        let mut rows = Vec::new();
        for key in settings::SESSION_KEYS {
            let Some(d) = settings::def(key) else { continue };
            if !settings::matches(find, d.label, key, d.help) {
                continue;
            }
            let mut under = Vec::new();
            if self.dash.config.as_ref().is_some_and(|c| c.exposes(key)) {
                under.push(
                    h_flex()
                        .mt(px(8.))
                        .gap(px(12.))
                        .items_center()
                        .child(mono("default for new sessions", 12., p.dim))
                        .child(div().flex_1())
                        .child(self.config_control(key, p, cx))
                        .into_any_element(),
                );
                if let Some(t) = self.applies(key) {
                    under.push(mono(t, 12., p.dim).into_any_element());
                }
                under.extend(self.config_under(key, p));
            }
            rows.push(Row {
                id: format!("session-{key}").into(),
                label: d.label.into(),
                key,
                help: d.help,
                control: Self::locked(self.session_value(key), p),
                tag: None,
                under,
            });
        }
        if settings::matches(find, "System prompt", "system_prompt", "system prompt") {
            let prompt = self.model.stats.view.as_ref().and_then(|v| v.system_prompt.clone());
            let well = match prompt {
                Some(text) => div()
                    .id("system-prompt")
                    .mt(px(8.))
                    .max_h(px(12. * 20.))
                    .overflow_y_scroll()
                    .p(px(10.))
                    .bg(p.code)
                    .font_family(crate::fonts::MONO)
                    .text_size(px(12.5))
                    .line_height(px(20.))
                    .text_color(p.code_fg)
                    .whitespace_normal()
                    .child(SharedString::from(text.to_string()))
                    .into_any_element(),
                None => mono("the daemon's default prompt", 12.5, p.dim).mt(px(4.)).into_any_element(),
            };
            rows.push(Row {
                id: "session-system".into(),
                label: "System prompt".into(),
                key: "system_prompt",
                help: "This session's system prompt (/prompt in the TUI). Applying and reloading from disk wait for the client's Set.",
                control: Self::locked(
                    if self.model.stats.view.as_ref().is_some_and(|v| v.system_prompt.is_some()) { "custom" } else { "default" },
                    p,
                ),
                tag: None,
                under: vec![well],
            });
        }
        if settings::def("cache_keepwarm").is_some_and(|d| settings::matches(find, "Keep-warm", d.key, d.help)) {
            rows.push(Row {
                id: "session-keepwarm".into(),
                label: "Keep-warm".into(),
                key: "keepwarm",
                help: "Pings the prompt cache so it stays warm (/keepwarm off, auto or <N>h). The current mode is not on the wire: set it with /keepwarm in the TUI.",
                control: Self::locked(crate::usage::NONE, p),
                tag: None,
                under: Vec::new(),
            });
        }
        let rows = rows.into_iter().enumerate().map(|(i, r)| self.render_row(r, i == 0, p, cx)).collect();
        (line.into(), rows)
    }

    fn group_config(&self, p: &Palette, cx: &Context<Self>, find: &str) -> (SharedString, Vec<AnyElement>) {
        let path =
            self.dash.config.as_ref().map(|c| c.path.clone()).filter(|s| !s.is_empty()).unwrap_or_else(|| "~/.synaps-cli/config".into());
        let local = self.dash.local == Some(true);
        let line = match (&self.dash.answer, self.dash.config.as_ref(), local) {
            (Answer::Up, Some(c), true) if c.writable && self.dash.config_writable() => format!("{path} · saved through synaps-dash"),
            (Answer::Up, Some(_), true) => format!("{path} · synaps-dash cannot write it (no lock): read-only"),
            (Answer::Up, Some(_), false) => format!("{path} on the daemon's machine · read-only from here"),
            (Answer::Down(Fail::Token), _, _) => "synaps-dash token changed: reconnect · read-only".into(),
            (_, _, true) => format!("read from {path} · read-only without synaps-dash"),
            _ => "synaps-dash is not reachable: values unknown".into(),
        };
        // The session rows' defaults show under them while a session is open.
        let session_open = self.model.tabs.current().is_some() && self.model.active.is_some();
        let mut out = Vec::new();
        for (cat, keys) in settings::CONFIG_GROUPS {
            let mut rows = Vec::new();
            for &key in keys {
                if session_open && settings::SESSION_KEYS.contains(&key) {
                    continue;
                }
                let Some(d) = settings::def(key) else { continue };
                if !settings::matches(find, d.label, key, d.help) {
                    continue;
                }
                rows.push(Row {
                    id: format!("config-{key}").into(),
                    label: d.label.into(),
                    key,
                    help: d.help,
                    control: self.config_control(key, p, cx),
                    tag: self.applies(key),
                    under: self.config_under(key, p),
                });
            }
            if !rows.is_empty() {
                out.push(Self::sub_head(cat.title(), p));
                out.extend(rows.into_iter().enumerate().map(|(i, r)| self.render_row(r, i == 0, p, cx)));
            }
        }
        (line.into(), out)
    }

    fn group_more(&self, p: &Palette, find: &str) -> (SharedString, Vec<AnyElement>) {
        let mut out = Vec::new();
        for cat in Cat::ORDER {
            let mut lines = Vec::new();
            for d in settings::reference(cat) {
                if !settings::matches(find, d.label, d.key, d.help) {
                    continue;
                }
                let value = match self.settings.reference.as_ref() {
                    Some(r) => settings::shown(d.key, r.get(d.key).map(String::as_str)),
                    None => crate::usage::NONE.into(),
                };
                let first = lines.is_empty();
                lines.push(
                    h_flex()
                        .w_full()
                        .min_h(px(32.))
                        .gap(px(12.))
                        .items_center()
                        .when(!first, |d| d.border_t_1().border_color(p.hair))
                        .child(sans(d.label, 14., 22., p.ink).flex_none())
                        .child(mono(d.key, 12., p.dim).flex_1().min_w_0().overflow_hidden().text_ellipsis())
                        .child(mono(value, 12.5, p.dim).flex_none().max_w(px(220.)).overflow_hidden().text_ellipsis())
                        .child(glyph(State::Locked, p))
                        .into_any_element(),
                );
            }
            if !lines.is_empty() {
                out.push(Self::sub_head(cat.title(), p));
                out.extend(lines);
            }
        }
        let n: usize = Cat::ORDER.iter().map(|&c| settings::reference(c).count()).sum();
        let source = if self.dash.local == Some(true) { "values from the local config" } else { "a remote daemon's values: —" };
        (format!("{n} keys · edit with /settings in the TUI on the daemon's machine · {source}").into(), out)
    }

    fn group_providers(&self, p: &Palette, find: &str) -> (SharedString, Vec<AnyElement>) {
        let rows: Vec<AnyElement> = match self.dash.config.as_ref() {
            Some(c) if !c.providers.is_empty() => c
                .providers
                .iter()
                .filter(|v| settings::matches(find, &v.name, &v.kind, &v.source))
                .enumerate()
                .map(|(i, v)| {
                    let mut s = format!("{} · from {}", v.kind, v.source);
                    if let Some(n) = &v.note {
                        s.push_str(&format!(" · {n}"));
                    }
                    Self::info_row_owned(v.name.clone(), s, i == 0, p)
                })
                .collect(),
            Some(_) if find.is_empty() => vec![sans("No providers configured.", 13., 20., p.dim).py(px(8.)).into_any_element()],
            None if find.is_empty() => vec![sans(crate::usage::NONE, 13., 20., p.dim).py(px(8.)).into_any_element()],
            _ => Vec::new(),
        };
        ("read-only · keys and installs are managed in the TUI · never a key".into(), rows)
    }

    fn group_plugins(&self, p: &Palette, find: &str) -> (SharedString, Vec<AnyElement>) {
        let rows: Vec<AnyElement> = match self.dash.config.as_ref() {
            Some(c) if !c.plugins.is_empty() => c
                .plugins
                .iter()
                .filter(|v| settings::matches(find, &v.name, &v.scope, &v.description))
                .enumerate()
                .map(|(i, v)| {
                    let s = format!(
                        "{}{} · {}",
                        if v.enabled { "enabled" } else { "disabled" },
                        v.version.as_deref().map(|x| format!(" · v{x}")).unwrap_or_default(),
                        v.scope
                    );
                    Self::info_row_owned(v.name.clone(), s, i == 0, p)
                })
                .collect(),
            Some(_) if find.is_empty() => vec![sans("No plugins installed.", 13., 20., p.dim).py(px(8.)).into_any_element()],
            None if find.is_empty() => vec![sans(crate::usage::NONE, 13., 20., p.dim).py(px(8.)).into_any_element()],
            _ => Vec::new(),
        };
        ("read-only · keys and installs are managed in the TUI".into(), rows)
    }

    fn info_row_owned(label: String, value: String, first: bool, p: &Palette) -> AnyElement {
        h_flex()
            .w_full()
            .min_h(px(36.))
            .py(px(8.))
            .gap(px(24.))
            .items_center()
            .when(!first, |d| d.border_t_1().border_color(p.hair))
            .child(sans(label, 14., 22., p.ink).w(px(180.)).flex_none().overflow_hidden().text_ellipsis())
            .child(mono(value, 12.5, p.dim).flex_1().min_w_0().whitespace_normal())
            .into_any_element()
    }
}
