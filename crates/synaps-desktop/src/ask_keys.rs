//! Answering agent cards from the keyboard (synaps-cards `docs/design.md`
//! v1.2, §5.1, §6.1 and §7; the D2b row of §9.3).
//!
//! - **The chip.** While the pane's session has open cards and none is
//!   entered, its composer says so: "◇ 2 questions · Ctrl+J". A click on
//!   it is Ctrl+J. A card arriving never takes the focus.
//! - **Ctrl+J** goes to what waits first in [`crate::waiting`]'s order,
//!   over this window's visible panes. Prompts come first, and keep their
//!   own path (`AppView::jump_to_card`). A card is *entered*: the pane
//!   holds the keys on its own focus (never the window's, whose keys
//!   answer a prompt), and a batch is frozen, the pane's cards in that
//!   order. Ctrl+J while a prompt holds the keys goes on to the cards, so
//!   the two take turns.
//! - **Activation.** The entry names one card ([`Painted`], its scope).
//!   Its keys act from the first frame that paints its ring in view. A key
//!   before that ends the entry, and a printable one is typed into the
//!   composer: nothing is answered.
//! - **Keys on an entered card.** `1`–`9` pick (a quick card: the answer);
//!   `0` skips; ↑ ↓ move the highlight, which starts on the default; Space
//!   ticks the highlighted option of a multi pick, or the check under the
//!   keys; Enter answers with the highlighted option, or sends a form; Tab
//!   and Shift+Tab move between the card and its fields; Esc goes back to
//!   the composer. Any other printable key goes to the composer, typed.
//!   Only new presses act: a repeat, or a key held from before, does
//!   nothing ([`Presses`], the app's `FoldKeys` rule).
//! - **In a field**, the field keeps its keys: digits, Space and arrows
//!   edit, and an IME composition owns every key. Enter in a one-line
//!   field goes forward: it sends a whole form, else moves to the next
//!   field. Tab moves on, and Esc goes back to the composer.
//! - **A batch** (F13): an answer by key enters the batch's next card that
//!   is still open; it activates at its own first paint. A click that
//!   answers ends the batch, and the keys go back to the composer.
//! - **It ends**, and the keys go back to the composer, when the card is
//!   resolved or rebound, scrolls out of view, the window goes inactive,
//!   the session or the tab changes, or the keys go elsewhere (another
//!   pane, an overlay, a click).
//!
//! Every answer goes through D2a's send ([`AppView::ask_send`]): one
//! `Submit` or `Steer` of `cards_spec::message`'s text on the card's
//! attachment, gated as a click is ([`AppView::ask_why`]). Nothing here
//! answers a prompt or touches the authorize card (§5.1).

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use cards_spec::{DefaultChoice, FieldKind, FieldValue, Mode, Reply};
use gpui_kit::*;
use synaps_client::cards::State;

use crate::app::{AppView, FoldKeys};
use crate::deliver::Painted;
use crate::pane::PaneId;
use crate::turns::Kind;
use crate::waiting::{Waiting, Waits};

actions!(ask_keys, [AskTab, AskTabBack]);

/// The key context of the pane's card-mode focus: Tab and Shift+Tab move
/// between the entered card's controls, ahead of the kit's focus cycling.
pub(crate) const ASK_CTX: &str = "AskCard";

/// A pane's card mode: the focus that holds its keys (made on the first
/// entry, then always drawn, so it is never lost to the window's), and
/// the card entered.
#[derive(Default)]
pub(crate) struct CardMode {
    pub(crate) focus: Option<FocusHandle>,
    pub(crate) entry: Option<Entry>,
    /// The last Enter in one of its inputs came mid-composition (read as
    /// the key came, before anything committed it): its `PressEnter` is
    /// the IME's.
    pub(crate) enter_composing: bool,
}

/// Where the keys are on the entered card: its options and buttons, or a
/// field (a check: Space ticks it; a text field: its input has the focus).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Spot {
    Body,
    Field(String),
}

/// An entered card: the token (its scope, `painted`), its batch, and
/// where the keys are on it.
pub(crate) struct Entry {
    pub(crate) session: String,
    pub(crate) painted: Painted,
    /// The batch, frozen at Ctrl+J: this pane's open cards in
    /// [`crate::waiting`]'s order, this one at `at`.
    pub(crate) batch: Vec<Painted>,
    pub(crate) at: usize,
    /// The highlighted option (Enter's), from the default.
    pub(crate) hi: Option<usize>,
    pub(crate) spot: Spot,
    /// A field to put the keys in once the card is painted: a form's
    /// first that needs filling.
    pub(crate) field: Option<String>,
    /// Its ring was painted in view: its keys act.
    pub(crate) shown: Rc<Cell<bool>>,
    /// Its ring was painted in view since the frame began (read and reset
    /// each frame: none, and it scrolled out of view).
    pub(crate) seen: Rc<Cell<bool>>,
}

/// Every key's presses, window-wide ([`FoldKeys`]): whether the last key
/// down was a new press, not a repeat or a key still held from before.
#[derive(Default)]
pub(crate) struct Presses {
    keys: HashMap<String, FoldKeys>,
    pub(crate) last: bool,
}

impl Presses {
    pub(crate) fn down(&mut self, key: &str, now: Instant, held: bool) {
        self.last = self.keys.entry(key.to_string()).or_default().press(now, held);
    }

    pub(crate) fn up(&mut self, key: &str, now: Instant) {
        if let Some(k) = self.keys.get_mut(key) {
            k.release(now);
        }
    }
}

/// A card's binding, as D3 names one to enter ([`AppView::enter_card`]):
/// the call that asked it and the card id the history binds it to.
// D3's (Shady #83 P2-7): until its chords and rows land, only tests call it.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Binding {
    pub(crate) tool_id: String,
    pub(crate) card: cards_spec::CardId,
}

/// What waits in a visible pane ([`crate::waiting`]'s key).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Here {
    Prompt,
    Card(PaneId, Painted),
}

/// The text a key types, if it types any (a letter, a digit, a space).
fn typed(ks: &Keystroke) -> Option<String> {
    ks.key_char.clone().filter(|c| !c.is_empty() && !c.chars().any(char::is_control))
}

/// The highlight a card starts on: its default, when it names one option.
fn start_on(doc: &cards_spec::CardDoc) -> Option<usize> {
    match &doc.default {
        Some(DefaultChoice::One(l)) => doc.options.iter().position(|o| &o.label == l),
        Some(DefaultChoice::Many(_)) if !doc.options.is_empty() => Some(0),
        _ if doc.multi && !doc.options.is_empty() => Some(0),
        _ => None,
    }
}

impl AppView {
    /// The open cards of store `live`'s session as this desk may answer
    /// them: bound, the latest call of their tool id, and no answer of
    /// this desk's on its way (its receipt is drawn instead). With their
    /// age (the fold's order).
    fn open_cards(&self, live: crate::live::LiveId) -> Vec<(Painted, u64)> {
        let l = &self.model.lives[live];
        if l.watch.focused().is_some() {
            return Vec::new();
        }
        let Some(s) = l.active.as_ref().map(|a| a.session_id.as_str()) else { return Vec::new() };
        let fold = l.transcript.cards();
        fold.cards()
            .iter()
            .filter(|c| c.state == State::Open && self.model.memos.entry(s, c).is_none())
            .filter_map(|c| {
                let painted = Painted { tool_id: c.tool_id.clone(), id: c.id.clone()?, doc: Arc::new(c.doc.clone()?), wait: c.wait };
                painted.find(fold).is_some().then_some((painted, c.at as u64))
            })
            .collect()
    }

    /// What waits on this window's visible panes, for [`crate::waiting`]:
    /// the focused pane at place 0, then the others in reading order. A
    /// pane showing a view (Settings, Usage) shows nothing that waits.
    pub(crate) fn waiting_here(&self, prompts: bool) -> Vec<Waiting<Here>> {
        let focused = self.panes.focused;
        let mut places = vec![focused];
        places.extend(self.reading_order().into_iter().filter(|id| *id != focused));
        let mut out = Vec::new();
        for (place, id) in places.into_iter().enumerate() {
            let Some(p) = self.panes.get(id) else { continue };
            let tabs = if p.id == self.panes.cur.id { &self.model.tabs } else { &p.parked.tabs };
            if tabs.view().is_some() {
                continue;
            }
            if prompts && !self.model.lives[p.live].transcript.pending_prompts().is_empty() {
                out.push(Waiting { waits: Waits::Prompt, place, age: 0, key: Here::Prompt });
            }
            for (painted, age) in self.open_cards(p.live) {
                let waits = if painted.wait { Waits::Wait } else { Waits::Open };
                out.push(Waiting { waits, place, age, key: Here::Card(id, painted) });
            }
        }
        out
    }

    /// Some card waits on a visible pane: Ctrl+J has somewhere to go.
    pub(crate) fn ask_waits(&self) -> bool {
        self.waiting_here(false).iter().any(|w| matches!(w.key, Here::Card(..)))
    }

    /// Ctrl+J, the chip, the palette's jump: when the first in the order
    /// is a card, it is entered, with its pane's batch (true). A prompt
    /// first, or nothing, is the prompt path's (false). While a prompt
    /// holds the keys, prompts are passed over: Ctrl+J again goes on to
    /// the cards.
    pub(crate) fn ask_jump(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let at_prompt = self.panes.cur.card_keys.get().is_some();
        let all = self.waiting_here(!at_prompt);
        let Some(first) = crate::waiting::order(all.clone()).into_iter().next() else { return false };
        let Here::Card(pane, _) = first.key else { return false };
        let batch: Vec<Painted> = crate::waiting::batch(all, first.place)
            .into_iter()
            .filter_map(|h| match h {
                Here::Card(_, p) => Some(p),
                Here::Prompt => None,
            })
            .collect();
        if pane != self.panes.focused {
            self.focus_pane(pane, crate::pane::KeysTo::Follow, cx);
        }
        self.in_pane(pane, |v| v.enter_ask(batch, 0, window, cx));
        true
    }

    /// One card entered, by name: D3's way in (Ctrl+Shift+J, a Waiting
    /// row in Ctrl+P, a tab dot, an OS notification's click), never a
    /// "first that waits" jump, and never a prompt (§5.1: those keep their
    /// own path). `binding` is checked again, now: shown in one of this
    /// window's visible panes as `session`'s, and still open there as this
    /// desk may answer it: the fold's current binding of its call, no
    /// answer of this desk's on its way (the memo), the pane not on a
    /// view or a worker's. Then it is entered as Ctrl+J enters one: its
    /// pane focused, its row brought into view, its keys live from its
    /// first paint (§7), and a batch frozen of it and then its pane's
    /// other cards in Ctrl+J's order ([`crate::waiting::batch_from`]).
    /// True if it was entered; false, and nothing changes, if not. A
    /// session in a background tab or another window is the caller's to
    /// bring forward first.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn enter_card(&mut self, session: &str, binding: &Binding, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let all = self.waiting_here(false);
        let shows = |pane: PaneId| {
            let live = self.panes.get(pane).map(|p| p.live);
            live.and_then(|l| self.model.lives[l].active.as_ref()).is_some_and(|a| a.session_id == session)
        };
        let found = all.iter().find_map(|w| match &w.key {
            Here::Card(pane, p) if p.tool_id == binding.tool_id && p.id == binding.card && shows(*pane) => {
                Some((*pane, w.place, w.key.clone()))
            }
            _ => None,
        });
        let Some((pane, place, first)) = found else {
            log::info!(target: "synaps_desktop::card", "card {}: not entered: not open on screen", binding.card);
            return false;
        };
        let batch: Vec<Painted> = crate::waiting::batch_from(all, place, first)
            .into_iter()
            .filter_map(|h| match h {
                Here::Card(_, p) => Some(p),
                Here::Prompt => None,
            })
            .collect();
        if pane != self.panes.focused {
            self.focus_pane(pane, crate::pane::KeysTo::Follow, cx);
        }
        self.in_pane(pane, |v| v.enter_ask(batch, 0, window, cx));
        true
    }

    /// Card `at` of `batch` entered in this pane: the keys on the pane's
    /// card focus, its row brought into view at the next frame, its keys
    /// live from its first paint.
    fn enter_ask(&mut self, batch: Vec<Painted>, at: usize, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(session), Some(painted)) = (self.card_session(), batch.get(at).cloned()) else { return };
        let doc = painted.doc.clone();
        // A form that needs filling: the keys go to its first field to fill.
        let draft = self.card_drafts.get(&(session.clone(), painted.tool_id.clone()));
        let field = doc
            .fields
            .iter()
            .find(|f| {
                let v = draft.and_then(|d| d.value(&doc, &f.id, cx)).unwrap_or_else(|| f.value.clone());
                f.kind != FieldKind::Check && !f.optional && v == FieldValue::Text(String::new())
            })
            .map(|f| f.id.clone());
        if let Some(e) = self.panes.cur.ask.entry.take()
            && let Some(row) = self.ask_row(&e.painted.tool_id)
        {
            self.remeasure_row(row);
        }
        log::info!(target: "synaps_desktop::card", "card {}: entered ({} of {})", painted.id, at + 1, batch.len());
        let focus = self.panes.cur.ask.focus.get_or_insert_with(|| cx.focus_handle()).clone();
        let tool = painted.tool_id.clone();
        self.panes.cur.ask.entry = Some(Entry {
            session,
            painted,
            batch,
            at,
            hi: start_on(&doc),
            spot: Spot::Body,
            field,
            shown: Rc::default(),
            seen: Rc::default(),
        });
        window.focus(&focus, cx);
        if let Some(row) = self.ask_row(&tool) {
            self.remeasure_row(row);
        }
        cx.notify();
    }

    /// Card mode ends in this pane; `refocus`: the keys go to its
    /// composer (never to the window, whose Esc denies a prompt).
    pub(crate) fn leave_ask(&mut self, refocus: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(e) = self.panes.cur.ask.entry.take() {
            log::info!(target: "synaps_desktop::card", "card {}: left", e.painted.id);
            if let Some(row) = self.ask_row(&e.painted.tool_id) {
                self.remeasure_row(row);
            }
        }
        if refocus {
            let c = self.panes.cur.composer.clone();
            c.update(cx, |s, cx| s.focus(window, cx));
        }
        cx.notify();
    }

    /// A click answered a card: it never chains (§6.1). A batch going in
    /// this pane ends (its mouse-down may have ended it already, the keys
    /// gone with it), and the keys go back to the composer: never left on
    /// the window's focus, whose keys answer a prompt.
    pub(crate) fn clicked_an_answer(&mut self, cx: &mut Context<Self>) {
        if let Some(e) = self.panes.cur.ask.entry.take() {
            log::info!(target: "synaps_desktop::card", "card {}: a click answered, the batch ends", e.painted.id);
            if let Some(row) = self.ask_row(&e.painted.tool_id) {
                self.remeasure_row(row);
            }
        }
        self.focus_composer(cx);
    }

    /// The display row of the card of call `tool` in this pane: its id.
    pub(crate) fn ask_row(&self, tool: &str) -> Option<u64> {
        self.ask_ix(tool).map(|ix| self.panes.cur.disp[ix].id)
    }

    fn ask_ix(&self, tool: &str) -> Option<usize> {
        let items = self.model.lives[self.panes.cur.live].transcript.items();
        self.panes.cur.disp.iter().position(|d| {
            d.kind == Kind::Ask
                && matches!(items.get(d.item).and_then(|i| i.card.as_deref()),
                    Some(synaps_client::cards::CardRow::Ask(a)) if a.tool_id == tool)
        })
    }

    /// The card `painted` is still open here, as this desk may answer it.
    fn ask_still_open(&self, painted: &Painted) -> bool {
        self.open_cards(self.panes.cur.live).iter().any(|(p, _)| p == painted)
    }

    /// The entered card, when its keys may answer it now: its session, row
    /// and scope. Not while its controls don't act ([`AppView::ask_why`]):
    /// a click would find no listener either.
    fn ask_target(&self) -> Option<(String, u64, Painted)> {
        let e = self.panes.cur.ask.entry.as_ref()?;
        let row = self.ask_row(&e.painted.tool_id)?;
        self.ask_why(&e.painted, Some(&e.session)).is_none().then(|| (e.session.clone(), row, e.painted.clone()))
    }

    /// The pane's keys are the entered card's: on its card focus, or in
    /// one of its fields.
    fn ask_holds_keys(&self, window: &Window, cx: &App) -> bool {
        let Some(e) = self.panes.cur.ask.entry.as_ref() else { return false };
        self.panes.cur.ask.focus.as_ref().is_some_and(|f| f.is_focused(window)) || self.ask_field_focused(e, window, cx).is_some()
    }

    /// The entered card's input with the keys: `Some(None)` its note,
    /// `Some(Some(id))` field `id`.
    fn ask_field_focused(&self, e: &Entry, window: &Window, cx: &App) -> Option<Option<String>> {
        let d = self.card_drafts.get(&(e.session.clone(), e.painted.tool_id.clone()))?;
        if d.note.as_ref().is_some_and(|i| i.read(cx).focus_handle(cx).is_focused(window)) {
            return Some(None);
        }
        let line = d.lines.iter().find(|(_, i)| i.read(cx).focus_handle(cx).is_focused(window)).map(|(id, _)| id.clone());
        let long = d.longs.iter().find(|(_, i)| i.read(cx).focus_handle(cx).is_focused(window)).map(|(id, _)| id.clone());
        line.or(long).map(Some)
    }

    /// An IME composition in the entered card's input that has the keys.
    fn ask_composing(&self, window: &mut Window, cx: &mut App) -> bool {
        self.panes.cur.ask.entry.is_some() && self.ime_composing(window, cx)
    }

    /// The input with the keys is mid IME composition (it has marked
    /// text): the pane's composer, or a card's field. Its keys are the
    /// IME's, Ctrl+J's included (§6.1: composition owns every key).
    pub(crate) fn ime_composing(&self, window: &mut Window, cx: &mut App) -> bool {
        let marked = |r: Option<std::ops::Range<usize>>| r.is_some();
        let c = &self.panes.cur.composer;
        if c.read(cx).focus_handle(cx).is_focused(window) {
            return marked(c.update(cx, |s, cx| EntityInputHandler::marked_text_range(s, window, cx)));
        }
        for d in self.card_drafts.values() {
            for i in d.note.iter().chain(d.lines.values()) {
                if i.read(cx).focus_handle(cx).is_focused(window) {
                    return marked(i.update(cx, |s, cx| EntityInputHandler::marked_text_range(s, window, cx)));
                }
            }
            for i in d.longs.values() {
                if i.read(cx).focus_handle(cx).is_focused(window) {
                    return marked(i.update(cx, |s, cx| EntityInputHandler::marked_text_range(s, window, cx)));
                }
            }
        }
        false
    }

    /// As a pane's frame begins: an entry whose card, session or keys went
    /// ends (§6.1: resolved, rebound, scrolled out of view, the window
    /// inactive, another tab or pane); one not painted yet is brought
    /// into view; one painted puts the keys in its first field to fill.
    pub(crate) fn sync_ask(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(e) = self.panes.cur.ask.entry.as_ref() else { return };
        let (shown, seen) = (e.shown.get(), e.seen.replace(false));
        let painted = e.painted.clone();
        let holds = self.ask_holds_keys(window, cx);
        let active = window.is_window_active();
        let here =
            self.model.tabs.view().is_none() && self.card_session().as_deref() == Some(e.session.as_str()) && self.ask_still_open(&painted);
        if holds && active && here && (!shown || seen) {
            if !shown {
                if let Some(ix) = self.ask_ix(&painted.tool_id) {
                    self.reveal_ask(ix, cx);
                }
            } else if let Some(id) = self.panes.cur.ask.entry.as_mut().and_then(|e| e.field.take()) {
                self.ask_go(&id, window, cx);
            }
            return;
        }
        // The keys went elsewhere: they stay there. Else back to the
        // composer (restoring the window lands there too).
        self.leave_ask(holds || !active, window, cx);
    }

    /// The card's row into view (row `ix`), as Ctrl+J brings a prompt's:
    /// under the sticky band, above the dock.
    fn reveal_ask(&mut self, ix: usize, cx: &mut Context<Self>) {
        let (top, bottom) = self.list_view();
        let top = top + self.sticky_inset(cx);
        let bottom = bottom - self.dock_growth();
        match self.panes.cur.list.bounds_for_item(ix) {
            Some(b) => {
                let (t, bt) = (f32::from(b.top()), f32::from(b.bottom()));
                let by = if t < top || bt - t > bottom - top {
                    t - top
                } else if bt > bottom {
                    bt - bottom
                } else {
                    0.
                };
                if by.abs() > 0.5 {
                    self.panes.cur.list.pause_following_tail();
                    self.panes.cur.scroller.touch();
                    self.panes.cur.list.scroll_by(px(by));
                }
            }
            None => {
                self.panes.cur.scroller.touch();
                self.panes.cur.list.scroll_to_reveal_item(ix);
            }
        }
    }

    /// A key down on the pane's card focus (card mode): see the module.
    pub(crate) fn ask_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let m = &ks.modifiers;
        // A chord is the app's: agent cards bind none.
        if m.control || m.alt || m.platform || m.function {
            return;
        }
        let text = typed(ks);
        let key = ks.key.as_str();
        let digit = (!m.shift && key.len() == 1).then(|| key.chars().next().and_then(|c| c.to_digit(10))).flatten();
        let card_key = digit.is_some() || matches!(key, "enter" | "escape" | "space" | "up" | "down");
        let shown = self.panes.cur.ask.entry.as_ref().map(|e| e.shown.get());
        if shown != Some(true) || !card_key {
            // Not entered (it ended under the keys), not painted yet, or a
            // letter: the key is the composer's, typed there; nothing is
            // answered. Enter, Esc and the arrows go nowhere.
            if text.is_some() || key == "escape" || shown.is_none() {
                cx.stop_propagation();
                self.leave_ask(true, window, cx);
                if let Some(t) = text {
                    let c = self.panes.cur.composer.clone();
                    c.update(cx, |s, cx| EntityInputHandler::replace_text_in_range(s, None, &t, window, cx));
                }
            } else if card_key {
                cx.stop_propagation();
            }
            return;
        }
        cx.stop_propagation();
        // Only a new press acts: a repeat, or a key held from before.
        if !self.ask_presses.last {
            log::debug!(target: "synaps_desktop::card", "card key {key}: not a new press");
            return;
        }
        match (key, digit) {
            ("escape", _) => self.leave_ask(true, window, cx),
            ("up" | "down", _) => self.ask_move(key == "down", cx),
            ("space", _) => self.ask_space(window, cx),
            ("enter", _) => self.ask_enter(window, cx),
            (_, Some(0)) => {
                if let Some((s, row, painted)) = self.ask_target()
                    && self.ask_send(&s, row, &painted, Reply::Skip, cx)
                {
                    self.ask_next(window, cx);
                }
            }
            (_, Some(n)) => self.ask_digit(n as usize - 1, window, cx),
            _ => {}
        }
    }

    /// `1`–`9`: option `i` (a quick card's answer; a form's pick).
    fn ask_digit(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((s, row, painted)) = self.ask_target() else { return };
        let Some(label) = painted.doc.options.get(i).map(|o| o.label.clone()) else { return };
        if let Some(e) = self.panes.cur.ask.entry.as_mut() {
            e.hi = Some(i);
            e.spot = Spot::Body;
        }
        if self.card_pick(&s, row, &painted, &label, window, cx) {
            self.ask_next(window, cx);
        }
    }

    /// Enter on the card: a quick card's highlighted option (or its Send,
    /// once an option that asks for a note is picked); a form's Send.
    fn ask_enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((s, row, painted)) = self.ask_target() else { return };
        let doc = painted.doc.clone();
        let noted = self
            .card_drafts
            .get(&(s.clone(), painted.tool_id.clone()))
            .is_some_and(|d| d.picked.iter().any(|l| doc.option(l).is_some_and(|o| o.note.is_some())));
        let sent = if doc.mode() == Mode::Quick && !noted {
            let hi = self.panes.cur.ask.entry.as_ref().and_then(|e| e.hi);
            let Some(label) = hi.and_then(|i| doc.options.get(i)).map(|o| o.label.clone()) else { return };
            self.card_pick(&s, row, &painted, &label, window, cx)
        } else {
            self.card_submit(&s, row, &painted, cx)
        };
        if sent {
            self.ask_next(window, cx);
        }
    }

    /// Space: the check under the keys ticks; on a multi pick, the
    /// highlighted option.
    fn ask_space(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((s, row, painted)) = self.ask_target() else { return };
        let Some(e) = self.panes.cur.ask.entry.as_ref() else { return };
        match e.spot.clone() {
            Spot::Field(id) => self.card_field(&s, row, &painted, &id, window, cx),
            Spot::Body => {
                let label = e.hi.filter(|_| painted.doc.multi).and_then(|i| painted.doc.options.get(i)).map(|o| o.label.clone());
                if let Some(l) = label {
                    self.card_pick(&s, row, &painted, &l, window, cx);
                }
            }
        }
    }

    /// ↓ (`down`) or ↑: the highlight moves among the options.
    fn ask_move(&mut self, down: bool, cx: &mut Context<Self>) {
        let Some(e) = self.panes.cur.ask.entry.as_mut() else { return };
        let n = e.painted.doc.options.len();
        if n == 0 {
            return;
        }
        e.hi = Some(match (e.hi, down) {
            (None, true) => 0,
            (None, false) => n - 1,
            (Some(i), true) => (i + 1).min(n - 1),
            (Some(i), false) => i.saturating_sub(1),
        });
        e.spot = Spot::Body;
        let tool = e.painted.tool_id.clone();
        if let Some(row) = self.ask_row(&tool) {
            self.remeasure_row(row);
        }
        cx.notify();
    }

    /// The batch's next card still open, entered; none, the composer.
    fn ask_next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(e) = self.panes.cur.ask.entry.take() else { return };
        match (e.at + 1..e.batch.len()).find(|k| self.ask_still_open(&e.batch[*k])) {
            Some(k) => self.enter_ask(e.batch, k, window, cx),
            None => {
                self.panes.cur.ask.entry = Some(e);
                self.leave_ask(true, window, cx);
            }
        }
    }

    /// Tab (or Shift+Tab, `back`) on the entered card: the next (previous)
    /// of its controls, round: the card itself, then each field.
    pub(crate) fn ask_tab(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(e) = self.panes.cur.ask.entry.as_ref() else { return };
        let mut stops = vec![None];
        stops.extend(e.painted.doc.fields.iter().map(|f| Some(f.id.clone())));
        let now = match self.ask_field_focused(e, window, cx) {
            Some(f) => f,
            None => match &e.spot {
                Spot::Body => None,
                Spot::Field(id) => Some(id.clone()),
            },
        };
        let i = stops.iter().position(|s| *s == now).unwrap_or(0);
        let n = stops.len();
        let to = stops[if back { (i + n - 1) % n } else { (i + 1) % n }].clone();
        match to {
            Some(id) => self.ask_go(&id, window, cx),
            None => {
                if let Some(e) = self.panes.cur.ask.entry.as_mut() {
                    e.spot = Spot::Body;
                }
                if let Some(f) = self.panes.cur.ask.focus.clone() {
                    window.focus(&f, cx);
                }
                self.ask_redraw(cx);
            }
        }
    }

    /// The keys to field `id` of the entered card: a check's spot, or a
    /// text field's input (opened, its value in it).
    fn ask_go(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some((s, row, painted)) = self.ask_target() else { return };
        let Some(f) = painted.doc.field(id) else { return };
        if let Some(e) = self.panes.cur.ask.entry.as_mut() {
            e.spot = Spot::Field(id.to_string());
        }
        if f.kind == FieldKind::Check {
            if let Some(h) = self.panes.cur.ask.focus.clone() {
                window.focus(&h, cx);
            }
            self.ask_redraw(cx);
        } else {
            self.card_field(&s, row, &painted, id, window, cx);
        }
    }

    fn ask_redraw(&mut self, cx: &mut Context<Self>) {
        if let Some(tool) = self.panes.cur.ask.entry.as_ref().map(|e| e.painted.tool_id.clone())
            && let Some(row) = self.ask_row(&tool)
        {
            self.remeasure_row(row);
        }
        cx.notify();
    }

    /// Tab or Shift+Tab in one of the entered card's inputs (the input's
    /// own Tab, captured): the card's next control. An IME composition
    /// keeps it.
    fn ask_field_tab(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) {
        let holds = self.panes.cur.ask.entry.as_ref().is_some_and(|e| self.ask_field_focused(e, window, cx).is_some());
        if holds && !self.ask_composing(window, cx) {
            cx.stop_propagation();
            self.ask_tab(back, window, cx);
        }
    }

    /// Esc in one of the entered card's inputs (the input's own, captured):
    /// back to the composer. An IME composition keeps it.
    fn ask_field_escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let holds = self.panes.cur.ask.entry.as_ref().is_some_and(|e| self.ask_field_focused(e, window, cx).is_some());
        if holds && !self.ask_composing(window, cx) {
            cx.stop_propagation();
            self.leave_ask(true, window, cx);
        }
    }

    /// Enter in a one-line input of row `row`'s card (its `PressEnter`),
    /// the card entered: forward. A whole form is sent; else the next
    /// field. An IME composition's Enter is the IME's.
    pub(crate) fn ask_field_enter(&mut self, row: u64, window: &mut Window, cx: &mut Context<Self>) {
        let composing = std::mem::take(&mut self.panes.cur.ask.enter_composing);
        let Some((s, r, painted)) = self.ask_target().filter(|(_, r, _)| *r == row) else { return };
        if composing || !self.ask_holds_keys(window, cx) || self.ask_composing(window, cx) {
            return;
        }
        let ready = self.card_drafts.get(&(s.clone(), painted.tool_id.clone())).is_some_and(|d| d.reply(&painted.doc, cx).is_ok());
        if ready {
            if self.card_submit(&s, r, &painted, cx) {
                self.ask_next(window, cx);
            }
        } else {
            self.ask_tab(false, window, cx);
        }
    }

    /// The composer's chip (§6.1): how many cards wait here, while none is
    /// entered in this pane.
    pub(crate) fn ask_chip(&self) -> Option<String> {
        if self.panes.cur.ask.entry.is_some() {
            return None;
        }
        let n = self.open_cards(self.panes.cur.live).len();
        (n > 0).then(|| format!("◇ {n} question{} · Ctrl+J", if n == 1 { "" } else { "s" }))
    }

    /// The focus was lost (an entered card's field went with its row)
    /// while a card is entered in the focused pane: card mode ends, and
    /// its composer has the keys, never the window, whose keys answer a
    /// prompt (true). Else the window's fallback (false).
    pub(crate) fn ask_focus_lost(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let focused = self.panes.focused;
        self.in_pane(focused, |v| {
            let entered = v.panes.cur.ask.entry.is_some();
            if entered {
                v.leave_ask(true, window, cx);
            }
            entered
        })
        .unwrap_or(false)
    }

    /// The pane's card-mode hooks, on its element: the focus that holds
    /// card mode's keys (zero-sized and always drawn once made: a focus
    /// lost hands the keys to the window's, which answer a prompt), the
    /// captures that give an entered card's inputs its Tab, Enter and Esc,
    /// and a watch painted after the list: an entered card it didn't paint
    /// in view (scrolled away) asks for one more frame, whose start ends
    /// the entry ([`AppView::sync_ask`]); a window at rest draws none.
    pub(crate) fn ask_hooks(&self, d: Div, cx: &mut Context<Self>) -> Div {
        let pane = self.panes.cur.id;
        let gone = self.panes.cur.ask.entry.as_ref().map(|e| {
            let (shown, seen) = (e.shown.clone(), e.seen.clone());
            canvas(
                |_, _, _| {},
                move |_, (), window, _| {
                    if shown.get() && !seen.get() {
                        window.request_animation_frame();
                    }
                },
            )
            .absolute()
            .size_0()
        });
        let focus = self.panes.cur.ask.focus.clone().map(|f| {
            div()
                .absolute()
                .size_0()
                .track_focus(&f)
                .key_context(ASK_CTX)
                .on_key_down(cx.listener(move |this, ev: &KeyDownEvent, window, cx| {
                    this.in_pane(pane, |this| this.ask_key(ev, window, cx));
                }))
                .on_action(cx.listener(move |this, _: &AskTab, window, cx| {
                    this.in_pane(pane, |this| this.ask_tab(false, window, cx));
                }))
                .on_action(cx.listener(move |this, _: &AskTabBack, window, cx| {
                    this.in_pane(pane, |this| this.ask_tab(true, window, cx));
                }))
        });
        d.children(focus)
            .children(gone)
            // Whether an Enter came mid-composition, as it comes: the
            // platform may commit the composition before its `PressEnter`.
            .capture_action(cx.listener(move |this, _: &gpui_kit::base::input::Enter, window, cx| {
                this.in_pane(pane, |this| this.panes.cur.ask.enter_composing = this.ask_composing(window, cx));
            }))
            .capture_action(cx.listener(move |this, _: &gpui_kit::base::input::IndentInline, window, cx| {
                this.in_pane(pane, |this| this.ask_field_tab(false, window, cx));
            }))
            .capture_action(cx.listener(move |this, _: &gpui_kit::base::input::OutdentInline, window, cx| {
                this.in_pane(pane, |this| this.ask_field_tab(true, window, cx));
            }))
            .capture_action(cx.listener(move |this, _: &gpui_kit::base::input::Escape, window, cx| {
                this.in_pane(pane, |this| this.ask_field_escape(window, cx));
            }))
    }
}
