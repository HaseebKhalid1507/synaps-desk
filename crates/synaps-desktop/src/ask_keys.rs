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
//! - **D3's ways in** (Ctrl+Shift+J, a WAITING row, a tab dot, the top
//!   bar's JUMP) name one card, and enter that card once they land on it
//!   ([`AppView::enter_card`], from `card_landed`): from the composer,
//!   another pane, another tab or session.
//! - **Activation.** The entry names one card ([`Painted`], its scope).
//!   Its keys act from the first frame that paints its ring in view. A key
//!   before that ends the entry, and a printable one is typed into the
//!   composer: nothing is answered.
//! - **Keys on an entered card.** `1`–`9` pick (a quick card: the answer);
//!   `0` skips; ↑ ↓ move the highlight, which starts on the default; Space
//!   ticks the highlighted option of a multi pick, or the check under the
//!   keys; Enter answers with the highlighted option, or sends a form; Tab
//!   and Shift+Tab go round the card, the note its pick asks for, and its
//!   fields; Esc goes back to the composer. Any other printable key goes
//!   to the composer, typed.
//! - **Only new, bare presses act**, on the card and in its fields alike
//!   ([`Presses::acts`]): a repeat, a key held from before, or a chord
//!   (Shift+Enter, Ctrl+Enter) does nothing. A press is a key-down after
//!   that key's key-up ([`Press`]), whether or not the platform says the
//!   key-down repeats (macOS loses that on Shift+Tab, Esc and the
//!   arrows). Every key-down counts, an input's action that took it too
//!   (a ↓ held in the composer, then Ctrl+J): each is stamped before any
//!   binding runs. So with Ctrl+J and Ctrl+Shift+J: held, it jumps once.
//!   A field's Enter, Esc and Tab are bindings of its input, which run
//!   before any key listener and never say whether the key repeats: Tab
//!   has none on an entered card (`NoAction`), and comes as a key; an
//!   Enter acts at its `PressEnter`, and an Esc at its key-down, only once
//!   that key-down said ([`FieldKey`]).
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
//! - **Changing an answer** (D4, §3.6, F15: Ctrl+J, Enter, a key). Last
//!   in Ctrl+J's order, once nothing waits on screen or off, come the
//!   answers this desk may change ([`AppView::changeable`]): the focused
//!   pane's latest first, then the others' in reading order; Ctrl+J on
//!   one entered goes to the next, round. A receipt is entered as a card
//!   is (its ring, its keys from its first paint): Enter reopens the
//!   card, the answer in it, and enters it, its highlight on the answer
//!   given ([`AppView::start_hi`]); Esc goes back; nothing else acts. On
//!   a reopened card the keys are an open card's, and what they send is
//!   a change; Esc keeps the answer as it was.
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

/// The key context of the pane's card-mode focus: Tab and Shift+Tab have
/// no binding there (`NoAction`: not the kit's focus cycling), so they
/// come to the pane as keys ([`AppView::ask_tab_key`]).
pub(crate) const ASK_CTX: &str = "AskCard";

/// The key context of an entered card ([`crate::ask`] draws it): Tab and
/// Shift+Tab in one of its inputs have no binding (not the input's
/// indent, nor the kit's focus cycling), and come as keys.
pub(crate) const ASK_ENTERED: &str = "AskEntered";

/// The entered card's inputs, for their `NoAction` Tab.
pub(crate) const ASK_INPUT_CTX: &str = "AskEntered > Input";

/// A pane's card mode: the focus that holds its keys (made on the first
/// entry, then always drawn, so it is never lost to the window's), and
/// the card entered.
#[derive(Default)]
pub(crate) struct CardMode {
    pub(crate) focus: Option<FocusHandle>,
    pub(crate) entry: Option<Entry>,
}

/// Where the keys are on the entered card, each a stop of Tab's round:
/// its options and buttons, the note its pick asks for (its input has the
/// focus), or a field (a check: Space ticks it; a text field: its input
/// has the focus).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Spot {
    Body,
    Note,
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
    /// Its receipt is entered (D4): Enter reopens the card to change its
    /// answer.
    pub(crate) receipt: bool,
}

/// Every key's presses, window-wide ([`Press`]): whether the last key-down
/// was a new press, not a repeat or a key still held from before; and
/// whether it may act on an entered card.
///
/// Every key-down is stamped once, before any binding runs
/// ([`Presses::intercept`], from the window's keystroke interceptor): GPUI
/// runs a key's bindings before any key listener, and an input's action
/// that takes the key (the composer's `MoveDown` for ↓) ends it there, so
/// the root's capture never hears that key-down (Shady #86 R3-1). The
/// root's capture ([`Presses::down`]) then reads that stamp, adding what
/// only the event says (whether it repeats), and stamps each key-up.
#[derive(Default)]
pub(crate) struct Presses {
    keys: HashMap<String, Press>,
    pub(crate) last: bool,
    /// The last key-down may act on an entered card: a new press, bare.
    pub(crate) acts: bool,
    /// An Enter or Esc in an input, from its action to its key-down (and
    /// an Enter's on to its `PressEnter`).
    pub(crate) field: Option<FieldKey>,
}

/// One key's presses, for an entered card's keys and Ctrl+J (Shady #86
/// R2-2): a key-down is a new press only if that key went up since its
/// last key-down, [`FoldKeys::PAIR`] or more before (X11's key-up right
/// before each repeat), and the key-down does not say it repeats. The
/// same on every platform. Not [`FoldKeys`]'s macOS rule, which takes a
/// key-down with no key-up between for a press when it does not say it
/// repeats (for Cmd+B, whose key-up AppKit drops): GPUI's macOS backend
/// rebuilds a non-printing key's key-down (Shift+Tab, Esc, the arrows)
/// that went through the input context's `doCommandBySelector:` with
/// `is_held: false`, so a held Shift+Tab would go round the card.
///
/// One exception, for the key-up AppKit drops when a key is let go while
/// Cmd is held: after a Cmd chord of the key (Cmd+2, a tab), its next
/// key-down that types text is a press, once (the one after it is not).
/// Else the `2` that answers a card after Cmd+2 would be lost. A key that
/// types text can come through that callback too, its repeat bit lost
/// (`gpui-pre-macos 0.3.12 window.rs:2759–2787`, Shady #86 P3-1): any key
/// mid IME composition; and a printable one (a digit, Space; not Enter or
/// Tab, control characters) while an IME source is active and the focused
/// input prefers it. A text key is no proof of a trustworthy repeat bit,
/// then; the exception is sound only where a card acts on such a key. On
/// the entered card itself, whose focus has no input handler, neither
/// case can happen: the key says when it repeats. In its fields, digits
/// and Space edit and never act, and an Enter or an Esc acts only with no
/// composition ([`FieldKey`]), the one way Enter comes through there.
/// Anything else that reads [`Presses::last`] for a text key would need
/// its own reason. A key-up this window never saw (it was let go in
/// another window) costs the next press of that key, once.
#[derive(Debug, Default)]
struct Press {
    down: Option<Instant>,
    up: Option<Instant>,
    /// The last key-down was a Cmd chord: its key-up may never come.
    cmd: bool,
    /// The last key-down as [`Presses::intercept`] stamped it (whether it
    /// was a new press), until the root's capture hears it. An input's
    /// action may have taken it: then nothing does, and the next key-down
    /// is stamped over it.
    unheard: Option<bool>,
}

impl Press {
    /// The key went down at `now` (`held`: it says it repeats; `cmd`: with
    /// Cmd; `text`: it types text): `true` for a new press.
    fn press(&mut self, now: Instant, held: bool, cmd: bool, text: bool) -> bool {
        let new = !held
            && match (self.down, self.up) {
                (None, _) => true,
                (Some(_), Some(up)) => now.saturating_duration_since(up) >= FoldKeys::PAIR,
                (Some(_), None) => self.cmd && text,
            };
        self.down = Some(now);
        self.up = None;
        self.cmd = cmd;
        new
    }

    fn release(&mut self, now: Instant) {
        if self.down.is_some() {
            self.up = Some(now);
        }
        self.unheard = None;
    }
}

/// An Enter or an Esc in an input, between its action and its key-down.
/// The input's binding runs before any key listener (GPUI dispatches a
/// key's bindings first), and its action ends an IME composition (Esc)
/// or emits `PressEnter`, an effect heard once the whole key-down is
/// through (Enter). So: the action says the key came, and whether
/// mid-composition (read then, before the input ends or commits it); its
/// own key-down, as [`Presses::down`] stamps it, whether it acts. An
/// entered card's field goes forward ([`AppView::ask_field_enter`]) or
/// back to the composer ([`AppView::ask_field_escape`]) only on that; a
/// key whose key-down never came (an action dispatched alone, or one the
/// input kept) or was another's acts on nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FieldKey {
    /// `"enter"` or `"escape"`.
    pub(crate) key: &'static str,
    pub(crate) composing: bool,
    pub(crate) acts: Option<bool>,
}

impl FieldKey {
    /// `key`'s, as it acts: a new, bare press, with no composition.
    fn acting(key: &'static str) -> Self {
        Self { key, composing: false, acts: Some(true) }
    }
}

/// A key a card takes: no modifier, but Shift on Tab (Shift+Tab). A chord
/// is the app's, or its input's (Shift+Enter, Ctrl+Enter): never a
/// card's answer.
fn bare(ks: &Keystroke) -> bool {
    let m = &ks.modifiers;
    !(m.control || m.alt || m.platform || m.function) && (!m.shift || ks.key == "tab")
}

impl Presses {
    /// Key `ks` went down at `now`, before any binding (the window's
    /// keystroke interceptor, [`AppView::intercept_keys`]): every key-down,
    /// whoever takes it. Whether it repeats, the root's capture says
    /// ([`Presses::down`]). A modifier alone (GPUI's keystroke for a tap of
    /// Shift, as it is let go) is no key-down.
    pub(crate) fn intercept(&mut self, ks: &Keystroke, now: Instant) {
        if ks.is_modifier_key() {
            return;
        }
        let (cmd, text) = (ks.modifiers.platform, ks.key_char.is_some());
        let p = self.keys.entry(ks.key.clone()).or_default();
        let new = p.press(now, false, cmd, text);
        p.unheard = Some(new);
    }

    /// Key `ks` went down at `now`, as the root's capture hears it (`held`:
    /// the platform says it is a repeat): the key-down stamped before its
    /// bindings ran, not stamped again. One never stamped (GPUI replays a
    /// key held back for a longer binding with no interceptor) is stamped
    /// here. A pending Enter's or Esc's key-down says whether it acts; any
    /// other key-down ends it.
    pub(crate) fn down(&mut self, ks: &Keystroke, now: Instant, held: bool) {
        let (cmd, text) = (ks.modifiers.platform, ks.key_char.is_some());
        let p = self.keys.entry(ks.key.clone()).or_default();
        self.last = match p.unheard.take() {
            Some(new) => new && !held,
            None => p.press(now, held, cmd, text),
        };
        self.acts = self.last && bare(ks);
        let acts = self.acts;
        self.field = self.field.take().filter(|f| ks.key == f.key && f.acts.is_none()).map(|f| FieldKey { acts: Some(acts), ..f });
    }

    pub(crate) fn up(&mut self, key: &str, now: Instant) {
        if let Some(k) = self.keys.get_mut(key) {
            k.release(now);
        }
    }
}

/// A card's binding, as D3 names one to enter ([`AppView::enter_card`]):
/// the call that asked it and the card id the history binds it to.
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

/// The highlight an open card starts on: its default, when it names one
/// option ([`AppView::start_hi`]).
fn start_on(doc: &cards_spec::CardDoc) -> Option<usize> {
    match &doc.default {
        Some(DefaultChoice::One(l)) => doc.options.iter().position(|o| &o.label == l),
        Some(DefaultChoice::Many(_)) if !doc.options.is_empty() => Some(0),
        _ if doc.multi && !doc.options.is_empty() => Some(0),
        _ => None,
    }
}

impl AppView {
    /// Every key-down of `window`, stamped before any binding runs
    /// ([`Presses::intercept`]): GPUI's keystroke interceptors run ahead of
    /// a key's actions (`gpui-pre 0.3.12 window.rs:6361`; its bindings at
    /// 6438), as the kit's own inputs use one to see the keys their
    /// actions take (`gpui-base 0.9.1 input/base/state.rs:943–953`). Every
    /// interceptor runs, whatever another did (`window.rs:2798–2817`).
    /// Another window's keys are its own.
    pub(crate) fn intercept_keys(window: &Window, cx: &mut Context<Self>) -> Subscription {
        let (me, at) = (cx.weak_entity(), window.window_handle());
        cx.intercept_keystrokes(move |ev, window, cx| {
            if window.window_handle() == at {
                let now = crate::app::input_now();
                let _ = me.update(cx, |v, _| v.ask_presses.intercept(&ev.keystroke, now));
            }
        })
    }

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

    /// The option an entered card's highlight starts on (Enter's): the one
    /// place it is set. Reopened to change its answer (D4), the answer
    /// given (its draft's first pick); else the agent's default
    /// ([`start_on`]).
    pub(crate) fn start_hi(&self, session: &str, painted: &Painted) -> Option<usize> {
        let doc = &painted.doc;
        if self.changing(session, &painted.tool_id) {
            let given = self.card_drafts.get(&(session.to_string(), painted.tool_id.clone())).and_then(|d| d.picked.first());
            if let Some(i) = given.and_then(|l| doc.options.iter().position(|o| &o.label == l)) {
                return Some(i);
            }
        }
        start_on(doc)
    }

    /// The answers this desk may change on this window's visible panes
    /// (D4, [`AppView::changeable`]), Ctrl+J's last: the focused pane's
    /// first, then the others' in reading order; in each, the latest
    /// first. A pane showing a view (Settings, Usage) shows none.
    fn changeable_here(&self) -> Vec<(PaneId, Painted)> {
        let focused = self.panes.focused;
        let mut places = vec![focused];
        places.extend(self.reading_order().into_iter().filter(|id| *id != focused));
        let mut out = Vec::new();
        for id in places {
            let Some(p) = self.panes.get(id) else { continue };
            let tabs = if p.id == self.panes.cur.id { &self.model.tabs } else { &p.parked.tabs };
            if tabs.view().is_some() {
                continue;
            }
            for c in self.model.lives[p.live].transcript.cards().cards().iter().rev() {
                let (Some(card), Some(doc)) = (c.id.clone(), c.doc.clone()) else { continue };
                let painted = Painted { tool_id: c.tool_id.clone(), id: card, doc: Arc::new(doc), wait: c.wait };
                if self.changeable(p.live, &painted).is_some() {
                    out.push((id, painted));
                }
            }
        }
        out
    }

    /// Ctrl+J with nothing waiting on screen or off (D4): an answer this
    /// desk may change, entered: the latest ([`AppView::changeable_here`]);
    /// with one entered, the next, round. Its receipt, whose Enter reopens
    /// it; a card already reopened, as a card. Whether one was.
    pub(crate) fn ask_jump_receipt(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let all = self.changeable_here();
        if all.is_empty() {
            return false;
        }
        let now = self.panes.cur.ask.entry.as_ref().map(|e| e.painted.clone());
        let focused = self.panes.focused;
        let at = now.and_then(|n| all.iter().position(|(pane, p)| *pane == focused && *p == n)).map_or(0, |i| (i + 1) % all.len());
        let (pane, painted) = all[at].clone();
        if pane != focused {
            self.focus_pane(pane, crate::pane::KeysTo::Follow, cx);
        }
        self.in_pane(pane, |v| {
            if v.card_session().is_some_and(|s| v.changing(&s, &painted.tool_id)) {
                v.enter_ask(vec![painted], 0, window, cx);
            } else {
                v.enter_receipt(painted, window, cx);
            }
        });
        true
    }

    /// An answer this desk may change is on a visible pane: Ctrl+J, with
    /// nothing waiting, has one to go to (D4, [`AppView::ask_jump_receipt`]).
    pub(crate) fn ask_changes(&self) -> bool {
        !self.changeable_here().is_empty()
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
    /// bring forward first: D3's jumps land first ([`AppView::card_landed`]).
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
        log::info!(target: "synaps_desktop::card", "card {}: entered ({} of {})", painted.id, at + 1, batch.len());
        let hi = self.start_hi(&session, &painted);
        let e =
            Entry { session, painted, batch, at, hi, spot: Spot::Body, field, shown: Rc::default(), seen: Rc::default(), receipt: false };
        self.put_entry(e, window, cx);
    }

    /// The receipt of the card `painted` entered in this pane (D4): Enter
    /// reopens it; its keys live from its first paint, as a card's.
    fn enter_receipt(&mut self, painted: Painted, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.card_session() else { return };
        log::info!(target: "synaps_desktop::card", "card {}: its receipt entered", painted.id);
        let e = Entry {
            session,
            painted: painted.clone(),
            batch: vec![painted],
            at: 0,
            hi: None,
            spot: Spot::Body,
            field: None,
            shown: Rc::default(),
            seen: Rc::default(),
            receipt: true,
        };
        self.put_entry(e, window, cx);
    }

    /// `e` entered in this pane, instead of what was: the keys on the
    /// pane's card focus, its row drawn again.
    fn put_entry(&mut self, e: Entry, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(e) = self.panes.cur.ask.entry.take()
            && let Some(row) = self.ask_row(&e.painted.tool_id)
        {
            self.remeasure_row(row);
        }
        let focus = self.panes.cur.ask.focus.get_or_insert_with(|| cx.focus_handle()).clone();
        let tool = e.painted.tool_id.clone();
        self.panes.cur.ask.entry = Some(e);
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

    /// The card `painted` is still as entered here: open, as this desk may
    /// answer it, or reopened to change its answer (D4); `receipt`: its
    /// receipt, not reopened, whose answer this desk may change.
    fn ask_still_open(&self, painted: &Painted, receipt: bool) -> bool {
        let live = self.panes.cur.live;
        let changing = self.card_session().is_some_and(|s| self.changing(&s, &painted.tool_id));
        if receipt || changing {
            return receipt != changing && self.changeable(live, painted).is_some();
        }
        self.open_cards(live).iter().any(|(p, _)| p == painted)
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

    /// The entered card's input with the keys: its note's, or field `id`'s.
    fn ask_field_focused(&self, e: &Entry, window: &Window, cx: &App) -> Option<Spot> {
        let d = self.card_drafts.get(&(e.session.clone(), e.painted.tool_id.clone()))?;
        if d.note.as_ref().is_some_and(|i| i.read(cx).focus_handle(cx).is_focused(window)) {
            return Some(Spot::Note);
        }
        let line = d.lines.iter().find(|(_, i)| i.read(cx).focus_handle(cx).is_focused(window)).map(|(id, _)| id.clone());
        let long = d.longs.iter().find(|(_, i)| i.read(cx).focus_handle(cx).is_focused(window)).map(|(id, _)| id.clone());
        line.or(long).map(Spot::Field)
    }

    /// An IME composition in the entered card's input that has the keys
    /// (the window's one rule: [`AppView::composing_now`]).
    fn ask_composing(&self, window: &mut Window, cx: &mut App) -> bool {
        self.panes.cur.ask.entry.is_some() && self.composing_now(window, cx)
    }

    /// As a pane's frame begins: an entry whose card, session or keys went
    /// ends (§6.1: resolved, rebound, scrolled out of view, the window
    /// inactive, another tab or pane); one not painted yet is brought
    /// into view; one painted puts the keys in its first field to fill.
    pub(crate) fn sync_ask(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(e) = self.panes.cur.ask.entry.as_ref() else { return };
        let (shown, seen, receipt) = (e.shown.get(), e.seen.replace(false), e.receipt);
        let painted = e.painted.clone();
        let holds = self.ask_holds_keys(window, cx);
        let active = window.is_window_active();
        let here = self.model.tabs.view().is_none()
            && self.card_session().as_deref() == Some(e.session.as_str())
            && self.ask_still_open(&painted, receipt);
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
            // Not laid out (above or below the view). GPUI's reveal only
            // anchors (`gpui-pre 0.3.12 elements/list.rs:680–709`; not
            // `scroll_to`, which stops following at 664–677): a list
            // following its tail would snap back to the end at its next
            // layout, the card never painted, its keys never live.
            None => {
                self.panes.cur.list.pause_following_tail();
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
        // Only a new, bare press acts: not a repeat, a key held from
        // before, nor a Shift+ one ([`Presses::acts`]).
        if !self.ask_presses.acts {
            log::debug!(target: "synaps_desktop::card", "card key {key}: not a new, bare press");
            return;
        }
        // An entered receipt (D4): Enter reopens its card; Esc goes back.
        // Nothing else acts on it.
        if self.panes.cur.ask.entry.as_ref().is_some_and(|e| e.receipt) {
            match key {
                "escape" => self.leave_ask(true, window, cx),
                "enter" => self.ask_reopen(window, cx),
                _ => {}
            }
            return;
        }
        match (key, digit) {
            ("escape", _) => self.ask_escape(window, cx),
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

    /// Enter on an entered receipt (D4): its card reopened, the answer in
    /// it, and entered: its keys live from its first paint, its highlight
    /// on the answer given ([`AppView::start_hi`]).
    fn ask_reopen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((s, row, painted)) = self.ask_target() else { return };
        if self.card_change(&s, row, &painted, window, cx) {
            self.enter_ask(vec![painted], 0, window, cx);
        }
    }

    /// Esc on an entered card, or in one of its fields: back to the
    /// composer; reopened to change its answer (D4), the answer stays.
    fn ask_escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((s, painted)) = self.panes.cur.ask.entry.as_ref().filter(|e| !e.receipt).map(|e| (e.session.clone(), e.painted.clone()))
            && self.card_keep(&s, &painted)
        {
            log::info!(target: "synaps_desktop::card", "card {}: kept, nothing sent", painted.id);
        }
        self.leave_ask(true, window, cx);
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
            Spot::Body | Spot::Note => {
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
        match (e.at + 1..e.batch.len()).find(|k| self.ask_still_open(&e.batch[*k], false)) {
            Some(k) => self.enter_ask(e.batch, k, window, cx),
            None => {
                self.panes.cur.ask.entry = Some(e);
                self.leave_ask(true, window, cx);
            }
        }
    }

    /// Tab (or Shift+Tab, `back`) on the entered card: the next (previous)
    /// of its stops, round: the card itself, the note its pick asks for
    /// (once open), then each field. What a note or a field holds stays.
    pub(crate) fn ask_tab(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) {
        // A receipt (D4) has no stops: its fields aren't drawn.
        let Some(e) = self.panes.cur.ask.entry.as_ref().filter(|e| !e.receipt) else { return };
        let note = self.ask_note(e);
        let mut stops = vec![Spot::Body];
        stops.extend(note.is_some().then_some(Spot::Note));
        stops.extend(e.painted.doc.fields.iter().map(|f| Spot::Field(f.id.clone())));
        let now = self.ask_field_focused(e, window, cx).unwrap_or_else(|| e.spot.clone());
        let i = stops.iter().position(|s| *s == now).unwrap_or(0);
        let n = stops.len();
        let to = stops[if back { (i + n - 1) % n } else { (i + 1) % n }].clone();
        match to {
            Spot::Field(id) => self.ask_go(&id, window, cx),
            Spot::Note => {
                if let Some(e) = self.panes.cur.ask.entry.as_mut() {
                    e.spot = Spot::Note;
                }
                if let Some(i) = note {
                    i.update(cx, |s, cx| s.focus(window, cx));
                }
                self.ask_redraw(cx);
            }
            Spot::Body => {
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

    /// The note the entered card's pick asks for, once open: its input.
    fn ask_note(&self, e: &Entry) -> Option<Entity<gpui_kit::component::input::InputState>> {
        let doc = &e.painted.doc;
        let d = self.card_drafts.get(&(e.session.clone(), e.painted.tool_id.clone()))?;
        let asks = !doc.multi && d.picked.iter().any(|l| doc.option(l).is_some_and(|o| o.note.is_some()));
        d.note.clone().filter(|_| asks)
    }

    /// Tab or Shift+Tab (`ks`) while the entered card holds the keys, on
    /// its body or in one of its inputs. No binding takes it there
    /// ([`ASK_CTX`], [`ASK_INPUT_CTX`]): it comes as a key, so only a new
    /// press of it moves, as any card key (§7), once the card is painted.
    /// An IME composition keeps it; a chord (Ctrl+Tab) is the app's.
    fn ask_tab_key(&mut self, ks: &Keystroke, window: &mut Window, cx: &mut Context<Self>) {
        let m = &ks.modifiers;
        if m.control || m.alt || m.platform || m.function || !self.ask_holds_keys(window, cx) || self.ask_composing(window, cx) {
            return;
        }
        cx.stop_propagation();
        let shown = self.panes.cur.ask.entry.as_ref().is_some_and(|e| e.shown.get());
        if !shown || !self.ask_presses.acts {
            log::debug!(target: "synaps_desktop::card", "card Tab: not a new press, or the card not painted yet");
            return;
        }
        self.ask_tab(m.shift, window, cx);
    }

    /// Esc in one of the entered card's inputs, at its key-down: after the
    /// input's `Escape` (which ends an IME composition) and the root's
    /// stamp ([`FieldKey`]). Back to the composer, only on a new, bare
    /// press whose action met no composition (Shady #86 R2-1): the Esc
    /// that ended one, its repeats and an Esc held from before do
    /// nothing, and nothing else hears them. An Esc the input kept (its
    /// own menu, say) never comes here.
    fn ask_field_escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(esc) = self.ask_presses.field.take_if(|f| f.key == "escape") else { return };
        let holds = self.panes.cur.ask.entry.as_ref().is_some_and(|e| self.ask_field_focused(e, window, cx).is_some());
        if !holds {
            return;
        }
        cx.stop_propagation();
        if esc != FieldKey::acting("escape") || self.ask_composing(window, cx) {
            log::debug!(target: "synaps_desktop::card", "field Esc {esc:?}: not a new, bare press, or a composition's");
            return;
        }
        self.ask_escape(window, cx);
    }

    /// An input's Enter or Esc (`key`), as its action comes: pending, till
    /// its key-down says whether it acts ([`FieldKey`]).
    fn ask_field_key(&mut self, key: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let composing = self.ask_composing(window, cx);
        self.ask_presses.field = Some(FieldKey { key, composing, acts: None });
    }

    /// Enter in a one-line input of row `row`'s card (its `PressEnter`),
    /// the card entered and painted: forward. A whole form is sent; else
    /// the next stop. Only an Enter whose own key-down was a new, bare
    /// press, and not an IME composition's ([`FieldKey`]): a repeat, an
    /// Enter held from before the keys came here, or Shift or Ctrl+Enter
    /// sends nothing and moves nothing.
    pub(crate) fn ask_field_enter(&mut self, row: u64, window: &mut Window, cx: &mut Context<Self>) {
        let enter = self.ask_presses.field.take();
        let Some((s, r, painted)) = self.ask_target().filter(|(_, r, _)| *r == row) else { return };
        if enter != Some(FieldKey::acting("enter")) {
            log::debug!(target: "synaps_desktop::card", "field Enter {enter:?}: not a new, bare press");
            return;
        }
        let shown = self.panes.cur.ask.entry.as_ref().is_some_and(|e| e.shown.get());
        if !shown || !self.ask_holds_keys(window, cx) || self.ask_composing(window, cx) {
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
            div().absolute().size_0().track_focus(&f).key_context(ASK_CTX).on_key_down(cx.listener(
                move |this, ev: &KeyDownEvent, window, cx| {
                    this.in_pane(pane, |this| this.ask_key(ev, window, cx));
                },
            ))
        });
        d.children(focus)
            .children(gone)
            // An Enter or an Esc in an input, as its action comes, before its
            // key-down ([`FieldKey`]); whether mid-composition is read now,
            // as the input ends a composition at its Esc, and the platform
            // may commit one before an Enter's `PressEnter`.
            .capture_action(cx.listener(move |this, _: &gpui_kit::base::input::Enter, window, cx| {
                this.in_pane(pane, |this| this.ask_field_key("enter", window, cx));
            }))
            .capture_action(cx.listener(move |this, _: &gpui_kit::base::input::Escape, window, cx| {
                this.in_pane(pane, |this| this.ask_field_key("escape", window, cx));
            }))
            // Tab and Shift+Tab on the entered card, its body or an input,
            // and Esc in an input: keys, after the root stamped them.
            .capture_key_down(cx.listener(move |this, ev: &KeyDownEvent, window, cx| {
                match ev.keystroke.key.as_str() {
                    "tab" => this.in_pane(pane, |this| this.ask_tab_key(&ev.keystroke, window, cx)),
                    "escape" => this.in_pane(pane, |this| this.ask_field_escape(window, cx)),
                    _ => None,
                };
            }))
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{Press, Presses};

    /// [`Press`]: a key-down is a press after the key's key-up, 30 ms or
    /// more before, and not when it says it repeats; never without a
    /// key-up between, however long after (a repeat that lost its held
    /// bit), unless the last key-down was a Cmd chord (its key-up AppKit
    /// drops) and this one types text.
    #[test]
    fn a_press_is_a_key_down_after_its_key_up() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let mut k = Press::default();
        assert!(k.press(t0, false, false, false), "the first key-down");
        for n in [10, 1_200, 3_600_000] {
            assert!(!k.press(ms(n), false, false, false), "no key-up between: a repeat, {n} ms on, held bit or not");
        }
        let at = 3_600_000;
        k.release(ms(at));
        assert!(!k.press(ms(at + 29), false, false, false), "29 ms after its key-up: X11's repeat");
        k.release(ms(at + 30));
        assert!(k.press(ms(at + 60), false, false, false), "30 ms after: a press");
        k.release(ms(at + 100));
        assert!(!k.press(ms(at + 200), true, false, false), "held: a repeat, after a key-up too");

        let mut two = Press::default();
        assert!(two.press(t0, false, true, false), "Cmd+2");
        assert!(two.press(ms(50), false, false, true), "then 2, its key-up lost under Cmd: a press");
        assert!(!two.press(ms(80), false, false, true), "and its next key-down, no key-up between: a repeat");
        let mut held = Press::default();
        held.press(t0, false, true, false);
        assert!(!held.press(ms(50), true, false, true), "after Cmd+2, a 2 that says it repeats: a repeat");
        let mut esc = Press::default();
        esc.press(t0, false, true, false);
        assert!(!esc.press(ms(50), false, false, false), "after Cmd+Esc, an Esc with no text and no key-up: a repeat");
    }

    fn key(k: &'static str) -> gpui_kit::Keystroke {
        gpui_kit::Keystroke::parse(k).expect("a keystroke").with_simulated_ime()
    }

    /// [`Presses`] (Shady #86 R3-1): each key-down is stamped once, before
    /// its bindings ([`Presses::intercept`]); the root's capture reads that
    /// stamp ([`Presses::down`]) and adds only what the event says. A
    /// key-down an input's action took (no capture hears it) still counts:
    /// the next one, with no key-up between, is a repeat. One the
    /// interceptor never saw (GPUI's replay of a key held back for a longer
    /// binding) is stamped by the capture. A lone modifier is no key-down.
    #[test]
    fn every_key_down_is_stamped_once_before_its_bindings() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let mut p = Presses::default();
        let down = key("down");
        p.intercept(&down, t0);
        p.down(&down, t0, false);
        assert!(p.last && p.acts, "a new press, stamped once (heard: not stamped again)");
        p.up("down", ms(10));
        p.intercept(&down, ms(100));
        assert!(p.keys["down"].unheard.is_some(), "stamped, not yet heard");
        p.intercept(&down, ms(600));
        p.down(&down, ms(600), false);
        assert!(!p.last, "the one before it an action took: no key-up since, a repeat");
        p.up("down", ms(700));
        p.intercept(&down, ms(800));
        p.down(&down, ms(800), true);
        assert!(!p.last, "it says it repeats");
        p.up("down", ms(900));
        p.down(&down, ms(1000), false);
        assert!(p.last, "never intercepted (a replay): stamped here, a press");
        p.down(&down, ms(1100), false);
        assert!(!p.last, "and not stamped twice: the next, no key-up between, a repeat");
        let shift = gpui_kit::Keystroke { key: "shift".into(), key_char: None, modifiers: Default::default() };
        p.intercept(&shift, ms(1200));
        assert!(!p.keys.contains_key("shift"), "a modifier alone: no key-down");
    }
}
