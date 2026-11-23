//! The composer foot's model and thinking pickers: a click on the model
//! name or the thinking level (or "Change model…" / "Change thinking…" in
//! the palette) opens the app's menu ([`crate::menu`]) over it, listing what
//! this session takes; a pick is `Set` on that session only. The daemon
//! lists the choices (`SessionQuery::ModelOptions`, feature
//! `model_options`); the label changes when the daemon's `setting_changed`
//! says so, never on the pick. A refusal shows in the open menu (and on the
//! composer's note line if it has closed).
//!
//! An older daemon has no list: the model menu shows the current model and
//! says so; the thinking menu lists the levels the settings page offers
//! ([`crate::settings::THINKING`]; the daemon refuses what the model lacks).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use gpui_kit::*;
use synaps_client::{ModelOptions, Setting};

use crate::app::AppView;
use crate::menu::{At, How, Item, Key, Menu, Phase};
use crate::pane::{PANES_MAX, PaneId};
use crate::theme::Palette;

/// Which picker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Model,
    Thinking,
}

impl Kind {
    pub fn setting(self, value: &str) -> Setting {
        match self {
            Kind::Model => Setting::Model { model: value.to_string() },
            Kind::Thinking => Setting::ReasoningLevel { level: value.to_string() },
        }
    }

    /// The wire name of its setting (`SettingApplied.setting`).
    pub fn wire(self) -> &'static str {
        match self {
            Kind::Model => "model",
            Kind::Thinking => "reasoning_level",
        }
    }

    fn noun(self) -> &'static str {
        match self {
            Kind::Model => "model",
            Kind::Thinking => "thinking",
        }
    }
}

/// An item that only says something (a note, the refusal): never picked.
pub(crate) const NOTE: usize = usize::MAX;
/// A provider's row (`PROVIDER + j`: `Target::providers[j]`): picking it
/// shows that provider's models in place of the ones shown.
const PROVIDER: usize = usize::MAX / 2;
/// One provider's models listed at most (the menu does not scroll); the
/// rest by `/model`.
const SHOWN: usize = 14;
/// A row's text at most, in characters (the menu is 360 px at most).
const CUT: usize = 44;

/// What an open picker is for and what it shows.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Target {
    pub kind: Kind,
    /// The pane it opened over, and that pane's attachment then: a pick goes
    /// to that attachment or nowhere.
    pub pane: PaneId,
    pub attach_seq: u64,
    /// The values (item `i` is `values[i]`), each with its label.
    pub values: Vec<(String, String)>,
    /// The session's value now, as the daemon last said.
    pub current: String,
    /// The daemon's list is on its way.
    pub loading: bool,
    /// Why nothing can be picked, or what the list lacks.
    pub note: Option<String>,
    /// Asked for, waiting for the daemon (its index).
    pub pending: Option<usize>,
    /// The daemon's refusal of the last ask: the value's index and why.
    pub refused: Option<(usize, String)>,
    /// Picks are off (read-only here, a turn running): why.
    pub off: Option<String>,
    /// More models than shown.
    pub more: usize,
    /// A model menu: every model the daemon listed (id, label, provider
    /// key). One provider's are the values; the others are rows of their own.
    pub all: Vec<(String, String, String)>,
    /// The providers' names by key (a row says the name).
    pub names: Vec<(String, String)>,
    /// The providers whose models are not shown, with how many each has.
    pub providers: Vec<(String, usize)>,
    /// The provider whose models are shown (its key).
    pub shown: Option<String>,
}

fn cut(s: &str) -> String {
    if s.chars().count() <= CUT { s.to_string() } else { format!("{}…", s.chars().take(CUT - 1).collect::<String>()) }
}

/// The same model, with or without a `provider/` prefix.
pub(crate) fn same_model(a: &str, b: &str) -> bool {
    let bare = |m: &str| m.rsplit_once('/').map_or(m, |(_, m)| m).to_string();
    a == b || bare(a) == bare(b)
}

impl Target {
    fn is_current(&self, v: &str) -> bool {
        match self.kind {
            Kind::Model => same_model(v, &self.current),
            Kind::Thinking => v == self.current,
        }
    }

    /// The current value's index.
    pub fn current_ix(&self) -> Option<usize> {
        self.values.iter().position(|(v, _)| self.is_current(v))
    }

    /// The values from the daemon's answer (`None`: it has none: an older
    /// daemon, or it could not answer). The current value is always one.
    /// A model menu shows the current model's provider's models.
    pub fn fill(&mut self, options: Option<&ModelOptions>) {
        self.loading = false;
        match (self.kind, options) {
            (Kind::Model, Some(o)) => {
                let key = |m: &synaps_client::ModelOption| {
                    if m.provider_key.is_empty() { m.provider.clone() } else { m.provider_key.clone() }
                };
                self.all = o
                    .models
                    .iter()
                    .map(|m| (m.id.clone(), if m.label.is_empty() { m.id.clone() } else { m.label.clone() }, key(m)))
                    .collect();
                self.names.clear();
                for m in &o.models {
                    if !self.names.iter().any(|(k, _)| *k == key(m)) {
                        self.names.push((key(m), m.provider.clone()));
                    }
                }
                // The daemon's word on the list (a block, a broker that did not answer).
                if o.notice.is_some() {
                    self.note = o.notice.clone();
                }
                self.show(None);
            }
            (Kind::Thinking, Some(o)) => self.set_values(o.thinking.iter().map(|l| (l.clone(), l.clone())).collect()),
            (Kind::Model, None) => {
                self.all.clear();
                self.show(None);
            }
            (Kind::Thinking, None) => self.set_values(crate::settings::THINKING.iter().map(|l| (l.to_string(), l.to_string())).collect()),
        }
    }

    /// The current model's provider.
    fn home(&self) -> Option<&str> {
        self.all.iter().find(|m| same_model(&m.0, &self.current)).map(|m| m.2.as_str())
    }

    /// A model menu shows `provider`'s models (`None`: the current model's,
    /// with the current model), the other providers as rows.
    pub fn show(&mut self, provider: Option<&str>) {
        let home = self.home().map(str::to_string);
        let shown = provider.map(str::to_string).or(home.clone()).or_else(|| self.all.first().map(|m| m.2.clone()));
        let mut providers: Vec<(String, usize)> = Vec::new();
        for (_, _, p) in &self.all {
            match providers.iter_mut().find(|(q, _)| q == p) {
                Some(e) => e.1 += 1,
                None => providers.push((p.clone(), 1)),
            }
        }
        providers.retain(|(p, _)| Some(p) != shown.as_ref());
        self.providers = providers;
        self.shown = shown.clone();
        let values = self.all.iter().filter(|m| Some(&m.2) == shown.as_ref()).map(|m| (m.0.clone(), m.1.clone())).collect();
        if shown == home || home.is_none() && provider.is_none() {
            self.set_values(values);
        } else {
            // Another provider's: the current model stays where it is.
            let current = std::mem::take(&mut self.current);
            self.set_values(values);
            self.current = current;
        }
    }

    fn set_values(&mut self, mut values: Vec<(String, String)>) {
        if !self.current.is_empty() && !values.iter().any(|(v, _)| self.is_current(v)) {
            values.insert(0, (self.current.clone(), self.current.clone()));
        }
        // Keep the current one when cutting.
        self.more = values.len().saturating_sub(SHOWN);
        if self.more > 0 {
            let cur = values.iter().position(|(v, _)| self.is_current(v));
            let keep = cur.filter(|&i| i >= SHOWN).map(|i| values[i].clone());
            values.truncate(SHOWN);
            if let Some(k) = keep {
                values[SHOWN - 1] = k;
            }
        }
        self.values = values;
    }

    /// Its rows: the refusal or note first, then the values (the current
    /// one marked, the one asked for marked while the daemon decides).
    pub fn items(&self) -> Vec<Item<usize>> {
        let mut out = Vec::new();
        if let Some((_, why)) = &self.refused {
            let mut it = Item::new(NOTE, cut(why)).off(why.clone());
            it.confirm = Some("REFUSED".into());
            out.push(it);
        }
        if let Some(n) = self.note.as_ref().or(self.off.as_ref()) {
            out.push(Item::new(NOTE, cut(n)).off(n.clone()));
        }
        if self.loading {
            out.push(Item::new(NOTE, "asking the daemon…").off("the session's list is on its way"));
        }
        // With other providers' rows below: whose models these are.
        if let Some(p) = self.shown.as_ref().filter(|_| !self.providers.is_empty()) {
            let name = self.name(p);
            out.push(Item::new(NOTE, name.to_uppercase()).off(format!("{name}'s models; the other providers below")));
        }
        for (i, (v, label)) in self.values.iter().enumerate() {
            let mut it = Item::new(i, cut(label));
            if self.pending == Some(i) {
                it = it.key("applying…");
            } else if self.is_current(v) {
                it = it.key("current");
            }
            if let Some(why) = &self.off {
                it = it.off(why.clone());
            } else if self.pending.is_some() && self.pending != Some(i) {
                it = it.off("waiting for the daemon's answer");
            }
            out.push(it);
        }
        if self.more > 0 {
            let more = format!("+{} more: /model provider/id", self.more);
            out.push(Item::new(NOTE, more.clone()).off(more));
        }
        let home = self.home();
        for (j, (p, n)) in self.providers.iter().enumerate() {
            let name = self.name(p);
            let mut it =
                Item::new(PROVIDER + j, cut(&format!("{name} · {n}"))).key(if home == Some(p.as_str()) { "current ▸" } else { "▸" });
            if self.pending.is_some() {
                it = it.off("waiting for the daemon's answer");
            }
            out.push(it);
        }
        out
    }

    /// A provider's name, by its key.
    fn name<'a>(&'a self, key: &'a str) -> &'a str {
        let name = self.names.iter().find(|(k, _)| k == key).map_or(key, |(_, name)| name.as_str());
        if name.is_empty() { "other" } else { name }
    }

    /// A provider's row: its provider.
    pub fn provider(&self, act: usize) -> Option<String> {
        act.checked_sub(PROVIDER).and_then(|j| self.providers.get(j)).map(|(p, _)| p.clone())
    }
}

/// The pickers' state on the window: one open at a time, as every menu.
#[derive(Default)]
pub(crate) struct Choices {
    pub menu: Option<(Menu<usize>, Target)>,
    was: Option<Menu<usize>>,
    leaving: Option<(Menu<usize>, Instant)>,
    /// Each pane's model and thinking labels as last drawn: where a menu
    /// opened from the keyboard sits.
    pub at: Rc<RefCell<[Slot; PANES_MAX]>>,
    /// The last list each attachment's daemon gave (shown while it is asked again).
    cache: Option<(u64, ModelOptions)>,
    /// The open menu's widest box so far: its least width while it is open
    /// (another provider's rows do not narrow it under the pointer).
    pub width: Rc<std::cell::Cell<f32>>,
}

impl Choices {
    /// The menu closed since the last frame leaves (fluid.md §3.10), as the
    /// other menus do.
    pub fn exits(&mut self, now: Instant, fx: &crate::motion::Motion) {
        match &self.menu {
            None => {
                if let Some(m) = self.was.take().filter(|m| m.born.is_some()) {
                    self.leaving = Some((m, now));
                }
            }
            Some((m, _)) => {
                self.leaving = None;
                if self.was.as_ref().map(|w| (w.born, &w.items)) != Some((m.born, &m.items)) {
                    self.was = Some(m.clone());
                }
            }
        }
        if self.leaving.as_ref().is_some_and(|(_, at)| fx.leave(*at).is_none()) {
            self.leaving = None;
        }
    }

    /// `t`'s menu at `at`: opened by the pointer at `born`, else by the
    /// keyboard, which selects the current value (not the first row).
    pub fn open(&mut self, t: Target, at: At, born: Option<Instant>) {
        let mut m = Menu::new(t.items(), at, born.is_none());
        // `refresh` places a keyboard menu's selection not on a value yet.
        m.sel = None;
        if let Some(b) = born {
            m = m.born(b);
        }
        self.width.set(0.);
        self.menu = Some((m, t));
        self.refresh();
    }

    /// The open menu's rows again, the selection kept on the same value.
    fn refresh(&mut self) {
        let Some((m, t)) = self.menu.as_mut() else { return };
        let was = m.sel.and_then(|i| m.items.get(i)).map(|it| it.act).filter(|a| *a != NOTE);
        m.items = t.items();
        m.sel = match was {
            Some(a) => m.items.iter().position(|it| it.act == a && it.enabled()),
            None if m.born.is_none() => {
                // From the keyboard: on the current value.
                let cur = t.current_ix();
                m.items.iter().position(|it| Some(it.act) == cur && it.enabled()).or(m.items.iter().position(Item::enabled))
            }
            None => None,
        };
    }
}

/// The composer foot's model name or thinking level, `el`: where its
/// picker opens from (its bounds are kept for the keyboard's), and with
/// `on`, a click opens it. Off over a worker's view (its facts, not the
/// session's).
pub(crate) fn label(
    el: Div,
    kind: Kind,
    on: bool,
    pane: PaneId,
    at: &Rc<RefCell<[Slot; PANES_MAX]>>,
    p: &Palette,
    cx: &Context<AppView>,
) -> AnyElement {
    let (slot, at) = (pane.index(), at.clone());
    let el = el.relative().child(
        canvas(
            move |b, _, _| {
                let mut a = at.borrow_mut();
                match kind {
                    Kind::Model => a[slot].0 = Some(b),
                    Kind::Thinking => a[slot].1 = Some(b),
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0(),
    );
    if !on {
        return el.into_any_element();
    }
    let ink = p.ink;
    let tip = match kind {
        Kind::Model => "Change model (this session)",
        Kind::Thinking => "Change thinking (this session)",
    };
    // No tooltip while a picker is open (it would sit on the menu's foot).
    let open = cx.entity().downgrade();
    el.id(("choice-label", slot * 2 + kind as usize))
        .debug_selector(move || format!("choice-{} {slot}", kind.noun()))
        .cursor_pointer()
        .hover(move |s| s.text_color(ink))
        .tooltip(move |window, cx| {
            if open.upgrade().is_some_and(|v| v.read(cx).choice.menu.is_some()) {
                return cx.new(|_| gpui::Empty).into();
            }
            crate::overlay::tip(tip, window, cx)
        })
        .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
            cx.stop_propagation();
            this.open_choice(kind, Some(ev.position()), window, cx);
        }))
        .into_any_element()
}

/// A pane's labels as last drawn: the model's, the thinking level's.
pub(crate) type Slot = (Option<Bounds<Pixels>>, Option<Bounds<Pixels>>);

impl AppView {
    /// Open the `kind` picker over this pane's label (`pt`: where the
    /// pointer pressed; `None`: from the keyboard).
    pub(crate) fn open_choice(&mut self, kind: Kind, pt: Option<Point<Pixels>>, window: &mut Window, cx: &mut Context<Self>) {
        let live = self.panes.cur.live;
        let pane = self.panes.cur.id;
        let l = &self.model.lives[live];
        let Some(attach_seq) = l.attachment() else {
            self.panes.cur.watch_note = Some(format!("{}: not attached to a session", kind.noun()).into());
            cx.notify();
            return;
        };
        let view = l.stats.view.as_ref();
        let current = match kind {
            Kind::Model => view
                .map(|v| v.model.clone())
                .filter(|m| !m.is_empty())
                .or(self.model.active_meta(live).map(|s| s.model.clone()))
                .unwrap_or_default(),
            Kind::Thinking => view.map(|v| v.effort.clone()).unwrap_or_default(),
        };
        let off = match l.input_gate() {
            Err(why) => Some(why.to_string()),
            Ok(()) if l.transcript.busy() => Some(format!("not while a turn runs: change the {} after it", kind.noun())),
            Ok(()) => None,
        };
        let supported = self.client.as_ref().is_some_and(|c| c.supports(synaps_client::FEATURE_MODEL_OPTIONS));
        let mut t = Target {
            kind,
            pane,
            attach_seq,
            values: Vec::new(),
            current,
            loading: false,
            note: None,
            pending: None,
            refused: None,
            off,
            more: 0,
            all: Vec::new(),
            providers: Vec::new(),
            names: Vec::new(),
            shown: None,
        };
        let cached = self.choice.cache.as_ref().filter(|c| c.0 == attach_seq).map(|c| c.1.clone());
        if supported {
            t.fill(cached.as_ref());
            t.loading = cached.is_none();
            if let Some(c) = &self.client
                && c.on(attach_seq).model_options().is_err()
            {
                t.loading = false;
            }
        } else {
            t.fill(None);
            if kind == Kind::Model {
                t.note = Some(match &self.client {
                    Some(_) => "this daemon does not list models: /model provider/id".into(),
                    None => "offline: no daemon to ask".into(),
                });
            }
        }
        self.close_menu();
        if let Some(prev) = window.focused(cx).filter(|f| *f != self.focus)
            && self.menu_focus_back.is_none()
        {
            self.menu_focus_back = Some(prev);
        }
        window.focus(&self.focus, cx);
        self.gear_menu = false;
        // Over its label: the pointer's press, else the label as last drawn.
        let label = {
            let at = self.choice.at.borrow();
            let (m, e) = at[pane.index()];
            match kind {
                Kind::Model => m,
                Kind::Thinking => e.or(m),
            }
        };
        let anchor = match (pt, label) {
            (_, Some(b)) => point(b.left(), b.top() - px(6.)),
            (Some(p), None) => p,
            (None, None) => {
                let (w, h) = self.centre.get();
                point(px(w / 2.), px(h - 80.))
            }
        };
        let born = pt.map(|_| self.fx.clock());
        self.choice.open(t, At::Point(anchor), born);
        cx.notify();
    }

    /// The daemon's list for `attach_seq` came (or why not).
    pub(crate) fn choice_options(&mut self, attach_seq: u64, reply: Result<ModelOptions, String>) {
        if let Ok(o) = &reply {
            self.choice.cache = Some((attach_seq, o.clone()));
        }
        let Some((_, t)) = self.choice.menu.as_mut().filter(|(_, t)| t.attach_seq == attach_seq) else { return };
        match &reply {
            Ok(o) => t.fill(Some(o)),
            Err(e) => {
                if t.values.is_empty() || t.kind == Kind::Model {
                    t.fill(None);
                }
                t.note = Some(e.clone());
            }
        }
        self.choice.refresh();
        self.model.view_dirty = true;
    }

    /// The daemon answered a pick on `attach_seq` (in that pane: this is
    /// its `apply_event`): applied, the menu closes (the label follows the
    /// session's `setting_changed`); refused, it says why, in the menu if it
    /// is still open, and on the composer's note line.
    pub(crate) fn choice_answered(&mut self, attach_seq: u64, setting: &str, reply: Result<Option<String>, String>) {
        let open = self.choice.menu.as_mut().filter(|(_, t)| t.attach_seq == attach_seq && t.kind.wire() == setting && t.pending.is_some());
        let noun = if setting == "model" { "model" } else { "thinking" };
        match reply {
            Ok(note) => {
                if open.is_some() {
                    self.close_menu();
                }
                // A clamp ("thinking → high (clamped from max …)") is worth saying.
                self.panes.cur.watch_note = note.map(SharedString::from);
                self.panes.cur.quiet_note = self.panes.cur.watch_note.clone();
            }
            Err(why) => {
                if let Some((_, t)) = open {
                    t.refused = t.pending.take().map(|i| (i, why.clone()));
                    self.choice.refresh();
                }
                self.panes.cur.watch_note = Some(format!("{noun} not changed: {why}").into());
                self.panes.cur.quiet_note = None;
            }
        }
        self.model.view_dirty = true;
    }

    /// An item picked: `Set` on the attachment the menu opened for.
    pub(crate) fn choice_pick(&mut self, i: usize, how: How, _: &mut Window, cx: &mut Context<Self>) {
        if !how.is_press() || i == NOTE {
            return;
        }
        let Some((m, t)) = self.choice.menu.as_mut() else { return };
        if t.pending.is_some() {
            return;
        }
        // A provider: its models in place of the ones shown (read-only too:
        // the list is worth reading).
        if let Some(p) = t.provider(i) {
            t.show(Some(&p));
            m.items = t.items();
            m.sel = matches!(how, How::Key { .. }).then(|| m.items.iter().position(|it| it.act < PROVIDER && it.act != NOTE)).flatten();
            cx.notify();
            return;
        }
        if t.off.is_some() {
            return;
        }
        let Some((value, _)) = t.values.get(i).cloned() else { return };
        if t.is_current(&value) {
            self.close_menu();
            cx.notify();
            return;
        }
        let (kind, seq) = (t.kind, t.attach_seq);
        let sent = self.client.as_ref().is_some_and(|c| c.on(seq).set(kind.setting(&value)).is_ok());
        if let Some((_, t)) = self.choice.menu.as_mut() {
            if sent {
                t.pending = Some(i);
                t.refused = None;
            } else {
                t.refused = Some((i, "the connection thread has stopped".into()));
            }
        }
        log::debug!(target: "synaps_desktop::send", "set {} {value}", kind.wire());
        self.choice.refresh();
        cx.notify();
    }

    /// A key while a picker is open: `true` if it took it.
    pub(crate) fn choice_keydown(&mut self, ks: &Keystroke, press: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some((m, _)) = self.choice.menu.as_mut() else { return false };
        match m.key(&ks.key, ks.modifiers.shift, press) {
            Key::Moved => {}
            Key::Pick(i) => {
                self.menu_picked = true;
                self.choice_pick(i, How::Key { press }, window, cx);
            }
            Key::Close => self.close_menu(),
            Key::Other => {
                self.close_menu();
                cx.notify();
                return false;
            }
        }
        cx.notify();
        true
    }

    /// Tab in the open picker.
    pub(crate) fn choice_tab(&mut self, back: bool) -> bool {
        let Some((m, _)) = self.choice.menu.as_mut() else { return false };
        m.key("tab", back, true);
        true
    }

    /// The open picker, over its label (it rises from it).
    pub(crate) fn render_choice_menu(&self, p: &Palette, cx: &Context<Self>) -> Option<impl IntoElement> {
        let (m, phase) = match self.choice.menu.as_ref() {
            Some((m, _)) => (m, Phase::enter(&self.fx, m.born, 1.)),
            None => {
                let (m, at) = self.choice.leaving.as_ref()?;
                (m, Phase::leave(&self.fx, *at, 1.)?)
            }
        };
        let At::Point(pt) = m.at else { return None };
        let width = self.choice.width.clone();
        let menu = crate::menu::render("choice-menu", m, p, AppView::choice_pick, |this, _| this.close_menu(), phase, &self.fx, cx)
            .min_w(px(width.get().max(236.)));
        // Its box, border included, measured as drawn.
        let body = div().relative().debug_selector(|| "choice-menu-box".into()).child(menu).child(
            canvas(
                move |b, _, _| {
                    let w = f32::from(b.size.width);
                    if w > width.get() {
                        width.set(w);
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0(),
        );
        Some(deferred(anchored().anchor(Anchor::BottomLeft).position(pt).snap_to_window_with_margin(px(8.)).child(body)).with_priority(3))
    }
}

/// The pickers in a window (`choice_tests.rs`).
#[cfg(all(test, unix))]
#[path = "choice_tests.rs"]
mod ui_tests;

#[cfg(test)]
mod tests {
    use super::{Choices, Kind, NOTE, PROVIDER, SHOWN, Target};
    use crate::menu::At;
    use crate::pane::PaneId;
    use gpui_kit::{point, px};
    use synaps_client::{ModelOption, ModelOptions};

    fn target(kind: Kind, current: &str) -> Target {
        Target {
            kind,
            pane: PaneId::FIRST,
            attach_seq: 1,
            values: Vec::new(),
            current: current.into(),
            loading: false,
            note: None,
            pending: None,
            refused: None,
            off: None,
            more: 0,
            all: Vec::new(),
            providers: Vec::new(),
            names: Vec::new(),
            shown: None,
        }
    }

    fn opts() -> ModelOptions {
        let m = |id: &str, label: &str, provider: &str| ModelOption {
            id: id.into(),
            label: label.into(),
            provider: provider.into(),
            provider_key: String::new(),
        };
        ModelOptions {
            model: "anthropic/claude-opus-5-5".into(),
            thinking_level: "high".into(),
            models: vec![
                m("openai-codex/gpt-5.5", "GPT-5.5", "OpenAI Codex"),
                m("anthropic/claude-sonnet-4-6", "Sonnet 4.6", "Anthropic"),
                m("anthropic/claude-opus-5-5", "Opus 5.5", "Anthropic"),
            ],
            thinking: vec!["off".into(), "adaptive".into(), "high".into(), "max".into()],
            notice: None,
        }
    }

    #[test]
    fn the_daemons_list_with_the_current_one_marked() {
        // A bare current id matches its provider-qualified row.
        let mut t = target(Kind::Model, "claude-opus-5-5");
        t.fill(Some(&opts()));
        let ids: Vec<&str> = t.values.iter().map(|v| v.0.as_str()).collect();
        assert_eq!(ids, ["anthropic/claude-sonnet-4-6", "anthropic/claude-opus-5-5"], "its provider's models");
        let items = t.items();
        // Headed by their provider (other providers' rows follow).
        assert_eq!((items[0].act, items[0].label.as_ref(), items[0].enabled()), (NOTE, "ANTHROPIC", false));
        assert_eq!(items[2].key, Some("current"));
        assert!(items[1..].iter().all(|i| i.enabled()));
        // The other providers are rows: one opens its models in place.
        assert_eq!((items[3].act, items[3].label.as_ref(), items[3].key), (PROVIDER, "OpenAI Codex · 1", Some("▸")));
        t.show(t.provider(items[3].act).as_deref());
        let items = t.items();
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_ref()).collect();
        assert_eq!(labels, ["OPENAI CODEX", "GPT-5.5", "Anthropic · 2"]);
        assert_eq!((items[1].key, items[2].key), (None, Some("current ▸")), "the current model is the other provider's");
        t.show(t.provider(items[2].act).as_deref());
        assert_eq!(t.current_ix(), Some(1), "back on the current one");
        let mut t = target(Kind::Thinking, "high");
        t.fill(Some(&opts()));
        assert_eq!(t.items().iter().filter(|i| i.key == Some("current")).map(|i| i.act).collect::<Vec<_>>(), [2]);
    }

    /// Sections are the daemon's provider keys, named by its names: two
    /// providers that share a name stay apart; its notice is a note row.
    #[test]
    fn sections_follow_the_daemons_provider_keys_and_notice() {
        let m = |id: &str, provider: &str, key: &str| ModelOption {
            id: id.into(),
            label: String::new(),
            provider: provider.into(),
            provider_key: key.into(),
        };
        let o = ModelOptions {
            model: "anthropic/claude-opus-5-5".into(),
            models: vec![
                m("anthropic/claude-opus-5-5", "Anthropic", "anthropic"),
                m("a:llm:x", "LLM", "a:llm"),
                m("b:llm:y", "LLM", "b:llm"),
            ],
            notice: Some("model change blocked: workers require collection/reconciliation".into()),
            ..ModelOptions::default()
        };
        let mut t = target(Kind::Model, "anthropic/claude-opus-5-5");
        t.fill(Some(&o));
        let items = t.items();
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_ref()).collect();
        assert_eq!(labels[1..], ["ANTHROPIC", "anthropic/claude-opus-5-5", "LLM · 1", "LLM · 1"], "{labels:?}");
        assert_eq!(items[0].act, NOTE);
        assert!(labels[0].starts_with("model change blocked"), "{labels:?}");
        t.show(t.provider(items[4].act).as_deref());
        assert_eq!(t.values, [("b:llm:y".to_string(), "b:llm:y".to_string())], "the second LLM's own");
    }

    /// From the keyboard (Ctrl+P, "Change thinking…"), the menu opens on
    /// the session's value, not the first row (finish gate F1); from the
    /// pointer, on nothing.
    #[test]
    fn a_keyboard_menu_opens_on_the_current_value() {
        let at = At::Point(point(px(0.), px(0.)));
        let sel = |c: &Choices| c.menu.as_ref().and_then(|(m, _)| m.sel.map(|i| m.items[i].label.to_string()));
        let mut t = target(Kind::Thinking, "low");
        t.fill(None);
        let mut c = Choices::default();
        c.open(t.clone(), at, None);
        assert_eq!(sel(&c).as_deref(), Some("low"), "not `off`");
        c.open(t, at, Some(std::time::Instant::now()));
        assert_eq!(sel(&c), None);
        let mut t = target(Kind::Model, "anthropic/claude-opus-5-5");
        t.fill(Some(&opts()));
        c.open(t, at, None);
        assert_eq!(sel(&c).as_deref(), Some("Opus 5.5"), "past the head and Sonnet");
    }

    /// The provider head shows only when other providers' rows follow
    /// (one provider's list needs no name), and is never picked.
    #[test]
    fn the_shown_provider_heads_its_models_only_among_others() {
        let mut t = target(Kind::Model, "anthropic/claude-opus-5-5");
        let mut o = opts();
        t.fill(Some(&o));
        assert_eq!(t.items().iter().filter(|i| i.label.as_ref() == "ANTHROPIC").count(), 1);
        o.models.retain(|m| m.provider == "Anthropic");
        t.fill(Some(&o));
        assert!(t.items().iter().all(|i| i.act != NOTE), "one provider: no head");
        let mut t = target(Kind::Thinking, "high");
        t.fill(Some(&opts()));
        assert!(t.items().iter().all(|i| i.act != NOTE), "levels: no head");
    }

    #[test]
    fn an_older_daemon_lists_no_models() {
        let mut t = target(Kind::Model, "anthropic/claude-opus-4-6");
        t.fill(None);
        assert_eq!(t.values, [("anthropic/claude-opus-4-6".to_string(), "anthropic/claude-opus-4-6".to_string())], "the current one only");
        let mut t = target(Kind::Thinking, "medium");
        t.fill(None);
        assert_eq!(t.values.len(), crate::settings::THINKING.len(), "the settings page's levels");
    }

    #[test]
    fn a_pick_waits_and_a_refusal_says_why() {
        let mut t = target(Kind::Thinking, "high");
        t.fill(Some(&opts()));
        t.pending = Some(3);
        let items = t.items();
        assert_eq!(items[3].key, Some("applying…"), "the label does not change; the row says it is asked");
        assert!(items.iter().filter(|i| i.act != 3).all(|i| !i.enabled()), "one ask at a time");
        t.refused = t.pending.take().map(|i| (i, "max is not supported by claude-opus-4-6".to_string()));
        let items = t.items();
        assert_eq!((items[0].act, items[0].confirm.as_deref()), (NOTE, Some("REFUSED")));
        assert!(items[0].label.contains("max is not supported"));
        assert_eq!(items[3].key, Some("current"), "still the daemon's");
    }

    #[test]
    fn read_only_rows_say_why_and_a_long_list_is_cut() {
        let mut t = target(Kind::Model, "anthropic/m19");
        t.off = Some("read-only: this window observes the session".into());
        let models = (0..20).map(|i| ModelOption {
            id: format!("anthropic/m{i}"),
            label: String::new(),
            provider: "Anthropic".into(),
            provider_key: "anthropic".into(),
        });
        t.fill(Some(&ModelOptions { models: models.collect(), ..ModelOptions::default() }));
        let items = t.items();
        assert!(items.iter().all(|i| !i.enabled()));
        assert_eq!(t.values.len(), SHOWN);
        assert_eq!(t.current_ix(), Some(SHOWN - 1), "the current one kept");
        assert!(items.last().unwrap().label.contains("+6 more"));
    }
}
