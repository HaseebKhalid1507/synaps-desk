//! Answering an agent card by mouse (synaps-cards `docs/design.md` v1.2,
//! §6.1 and §7; the D2a row of §9.3): what a click on a card does, and the
//! answer being made.
//!
//! - **A quick card** (options, one pick, no fields): one click on an
//!   option answers it (F1, F2). An option that asks for a note opens its
//!   note under it, and Send sends (F3).
//! - **A form** (several picks, or fields): a click picks or unpicks an
//!   option, ticks a check, or opens a field to type in; Send sends (F4,
//!   F5, F6). Its options start on the agent's default, its fields on
//!   their prefills, so a form prefilled and valid is one click.
//! - **Skip** on any open card (F7).
//! - **Retry** on an answer not sent, **Resend** on one the desk couldn't
//!   confirm (F16, F17, F20, F22): a human's click, never automatic.
//!
//! Every control acts only once armed ([`crate::app::ARM`] after it was
//! first painted in view in its pane, actionable: [`crate::ask::probe`]):
//! a click that lands as the card arrives and shifts the layout does
//! nothing (§7's negative test), nor one on a control that comes into view
//! (scrolled in, bound, confirmed) under a pointer already there (Shady
//! #81 P2-3). What a click
//! sends is checked on the click, not as painted ([`crate::deliver`]):
//! the window may send, the history is settled, the card is the one
//! painted. If not, nothing goes and the card's line says why.
//!
//! An answer never resolves a permission prompt (§5.1): its one effect is
//! a `Submit` or `Steer` of the text `cards_spec::message` built, on the
//! card's attachment ([`AppView::send_card`]).
//!
//! The keys (D2b, [`crate::ask_keys`]) act through the same paths as a
//! click, without its arm (theirs is the card's first paint): a pick, a
//! field, Send, Skip, all through [`AppView::ask_send`]. A click that
//! answers ends a keyboard batch, and the keys go back to the composer
//! ([`AppView::clicked_an_answer`]).

use std::collections::HashMap;
use std::time::Instant;

use cards_spec::{CardDoc, FieldKind, FieldValue, Mode, Reply};
use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::*;
use serde_json::{Map, Value, json};

use crate::app::AppView;
use crate::deliver::{Outgoing, Painted};
use crate::turns::Kind;

/// A card's answer being made: what is picked, the note and the fields as
/// typed. Kept by session and call ([`AppView::card_drafts`]) until it is
/// sent.
pub(crate) struct Draft {
    /// The labels picked, in the doc's order.
    pub(crate) picked: Vec<String>,
    /// The note's field, once an option that asks for one is picked.
    pub(crate) note: Option<Entity<InputState>>,
    /// Each text field's input, once clicked into (until then, its prefill).
    pub(crate) lines: HashMap<String, Entity<InputState>>,
    /// Each long field's input, once clicked into.
    pub(crate) longs: HashMap<String, Entity<TextareaState>>,
    /// Each check's value, once clicked (until then, its prefill).
    pub(crate) checks: HashMap<String, bool>,
    /// What the last click met, when it sent nothing.
    pub(crate) said: Option<String>,
    _subs: Vec<Subscription>,
}

impl Draft {
    /// A fresh draft of `doc`: a form's options on the agent's default
    /// (one click sends a prefilled, valid form, F4); a quick card's on
    /// nothing (a click is the answer).
    pub(crate) fn new(doc: &CardDoc) -> Self {
        Draft {
            picked: default_picks(doc),
            note: None,
            lines: HashMap::new(),
            longs: HashMap::new(),
            checks: HashMap::new(),
            said: None,
            _subs: Vec::new(),
        }
    }

    /// Field `id`'s value as it stands: typed, or the prefill.
    pub(crate) fn value(&self, doc: &CardDoc, id: &str, cx: &App) -> Option<FieldValue> {
        let f = doc.field(id)?;
        Some(match f.kind {
            FieldKind::Check => FieldValue::Check(self.checks.get(id).copied().unwrap_or(f.value == FieldValue::Check(true))),
            FieldKind::Text => match self.lines.get(id) {
                Some(e) => FieldValue::Text(e.read(cx).value().to_string()),
                None => f.value.clone(),
            },
            FieldKind::Long => match self.longs.get(id) {
                Some(e) => FieldValue::Text(e.read(cx).value().to_string()),
                None => f.value.clone(),
            },
        })
    }

    /// The answer's values as the spec reads them (§1.4): the pick(s), the
    /// note of an option that asks for one, every field.
    fn values(&self, doc: &CardDoc, cx: &App) -> Value {
        let mut o = Map::new();
        if !doc.options.is_empty() {
            let picked: Vec<&str> = doc.options.iter().filter(|op| self.picked.contains(&op.label)).map(|op| op.label.as_str()).collect();
            if doc.multi {
                o.insert("choice".into(), json!(picked));
            } else if let Some(l) = picked.first() {
                o.insert("choice".into(), json!(l));
                if doc.option(l).is_some_and(|op| op.note.is_some()) {
                    let note = self.note.as_ref().map(|e| e.read(cx).value().to_string()).unwrap_or_default();
                    o.insert("note".into(), json!(note));
                }
            }
        }
        if !doc.fields.is_empty() {
            let mut fields = Map::new();
            for f in &doc.fields {
                let v = match self.value(doc, &f.id, cx) {
                    Some(FieldValue::Check(b)) => json!(b),
                    Some(FieldValue::Text(s)) => json!(s),
                    None => continue,
                };
                fields.insert(f.id.clone(), v);
            }
            o.insert("fields".into(), Value::Object(fields));
        }
        Value::Object(o)
    }

    /// The reply it makes, checked as the extension and every desk's fold
    /// check one (`cards_spec::parse_answer`), or what it still needs.
    pub(crate) fn reply(&self, doc: &CardDoc, cx: &App) -> Result<Reply, String> {
        cards_spec::parse_answer(doc, &self.values(doc, cx)).map(Reply::Answer).map_err(|e| needs(doc, &e))
    }
}

/// What a card's draft starts on: a form's options on the agent's
/// default; a quick card's on nothing.
pub(crate) fn default_picks(doc: &CardDoc) -> Vec<String> {
    match (doc.mode(), &doc.default) {
        (Mode::Form, Some(cards_spec::DefaultChoice::One(l))) => vec![l.clone()],
        (Mode::Form, Some(cards_spec::DefaultChoice::Many(ls))) => ls.clone(),
        _ => Vec::new(),
    }
}

/// What a form still needs, in its own words: the field by its label, the
/// note by its prompt.
fn needs(doc: &CardDoc, e: &cards_spec::Error) -> String {
    let shown = |s: &str| crate::ask::shown(s, cards_spec::Lines::Single);
    let path = e.path();
    if let Some(id) = path.strip_prefix("fields.") {
        let label = doc.field(id).map_or(id, |f| f.label.as_str());
        return match e.message() {
            "required" => format!("{} is needed", shown(label)),
            m => format!("{}: {m}", shown(label)),
        };
    }
    match (path, e.message()) {
        ("choice", "required") => "Pick an option".into(),
        ("note", "required for this option" | "must not be empty") => {
            let prompt = doc.options.iter().find_map(|o| o.note.as_deref()).unwrap_or("A note");
            format!("{} is needed", shown(prompt))
        }
        (p, m) => format!("{p}: {m}"),
    }
}

impl AppView {
    /// The session of the pane acted on, when a click there may answer its
    /// cards: attached, and showing the session's own transcript (not a
    /// worker's view).
    pub(crate) fn card_session(&self) -> Option<String> {
        let l = &self.model.lives[self.panes.cur.live];
        if l.watch.focused().is_some() {
            return None;
        }
        l.active.as_ref().map(|a| a.session_id.clone())
    }

    /// Control `what` of the card `painted` (row `row`) may act: first
    /// painted in view, actionable, in the pane acted on at least
    /// [`crate::app::ARM`] ago.
    fn ask_armed(&self, row: u64, painted: &Painted, what: &str) -> bool {
        let key = (row, crate::ask::control(&painted.id, what));
        let shown = self.panes.cur.ask_shown.borrow().get(&key).copied();
        shown.is_some_and(|at| crate::app::arm_now().saturating_duration_since(at) >= crate::app::ARM)
    }

    /// The draft of the card `painted` of `session`, made if there is none.
    fn draft(&mut self, session: &str, painted: &Painted) -> &mut Draft {
        self.card_drafts.entry((session.to_string(), painted.tool_id.clone())).or_insert_with(|| Draft::new(&painted.doc))
    }

    /// A click on option `label` of the card `painted` (row `row`): a quick
    /// card's answer, or a pick.
    pub(crate) fn ask_pick(&mut self, row: u64, painted: &Painted, label: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.card_session().filter(|_| self.ask_armed(row, painted, &format!("option {label}"))) else {
            log::debug!(target: "synaps_desktop::card", "card {}: a click before it armed, nothing sent", painted.id);
            return;
        };
        if self.card_pick(&session, row, painted, label, window, cx) {
            self.clicked_an_answer(cx);
        }
    }

    /// Option `label` of the card `painted` of `session`, by a click or a
    /// key (D2b): a quick card's answer (true: it went), or a pick.
    pub(crate) fn card_pick(
        &mut self,
        session: &str,
        row: u64,
        painted: &Painted,
        label: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let doc = painted.doc.clone();
        let Some(op) = doc.option(label) else { return false };
        if doc.mode() == Mode::Quick && op.note.is_none() {
            let reply =
                Reply::Answer(cards_spec::Answer { choice: Some(cards_spec::Choice::One(label.to_string())), note: None, fields: None });
            return self.ask_send(session, row, painted, reply, cx);
        }
        let session = session.to_string();
        let asks_note = op.note.clone();
        let d = self.draft(&session, painted);
        d.said = None;
        if doc.multi {
            match d.picked.iter().position(|l| l == label) {
                Some(i) => {
                    d.picked.remove(i);
                }
                None => d.picked.push(label.to_string()),
            }
        } else {
            d.picked = vec![label.to_string()];
        }
        if let Some(prompt) = asks_note
            && !doc.multi
        {
            let fresh = d.note.is_none();
            if fresh {
                let e = cx.new(|cx| InputState::new(window, cx).placeholder(crate::ask::shown(&prompt, cards_spec::Lines::Single)));
                let sub = Self::watch_input(row, &e, window, cx);
                let d = self.draft(&session, painted);
                d._subs.push(sub);
                d.note = Some(e);
            }
            if let Some(e) = self.draft(&session, painted).note.clone() {
                e.update(cx, |s, cx| s.focus(window, cx));
            }
        }
        self.remeasure_row(row);
        cx.notify();
        false
    }

    /// A click on field `id` of the card `painted`: a check ticks; a text
    /// field opens to type in, its prefill in it.
    pub(crate) fn ask_field(&mut self, row: u64, painted: &Painted, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.card_session().filter(|_| self.ask_armed(row, painted, &format!("field {id}"))) else { return };
        self.card_field(&session, row, painted, id, window, cx);
    }

    /// Field `id` of the card `painted` of `session`, by a click or a key
    /// (D2b): a check ticks; a text field opens to type in, its prefill in
    /// it, and has the keys.
    pub(crate) fn card_field(&mut self, session: &str, row: u64, painted: &Painted, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let session = session.to_string();
        let doc = painted.doc.clone();
        let Some(f) = doc.field(id) else { return };
        let now = self.draft(&session, painted).value(&doc, id, cx);
        match (f.kind, now) {
            (FieldKind::Check, Some(FieldValue::Check(b))) => {
                let d = self.draft(&session, painted);
                d.checks.insert(id.to_string(), !b);
                d.said = None;
            }
            (FieldKind::Text, Some(FieldValue::Text(v))) => {
                if !self.draft(&session, painted).lines.contains_key(id) {
                    let placeholder = f.placeholder.as_deref().map(|s| crate::ask::shown(s, cards_spec::Lines::Single)).unwrap_or_default();
                    let e = cx.new(|cx| {
                        let mut s = InputState::new(window, cx).placeholder(placeholder);
                        s.set_value(v, window, cx);
                        s
                    });
                    let sub = Self::watch_input(row, &e, window, cx);
                    let d = self.draft(&session, painted);
                    d._subs.push(sub);
                    d.lines.insert(id.to_string(), e);
                }
                if let Some(e) = self.draft(&session, painted).lines.get(id).cloned() {
                    e.update(cx, |s, cx| s.focus(window, cx));
                }
            }
            (FieldKind::Long, Some(FieldValue::Text(v))) => {
                if !self.draft(&session, painted).longs.contains_key(id) {
                    let placeholder = f.placeholder.as_deref().map(|s| crate::ask::shown(s, cards_spec::Lines::Single)).unwrap_or_default();
                    let e = cx.new(|cx| {
                        let mut s = TextareaState::new(window, cx).auto_grow(3, 10).placeholder(placeholder);
                        s.set_value(v, window, cx);
                        s
                    });
                    let sub = cx.subscribe_in(&e, window, move |this: &mut Self, _, ev: &InputEvent, _, cx| {
                        if matches!(ev, InputEvent::Change) {
                            this.remeasure_row(row);
                            cx.notify();
                        }
                    });
                    let d = self.draft(&session, painted);
                    d._subs.push(sub);
                    d.longs.insert(id.to_string(), e);
                }
                if let Some(e) = self.draft(&session, painted).longs.get(id).cloned() {
                    e.update(cx, |s, cx| s.focus(window, cx));
                }
            }
            _ => return,
        }
        self.remeasure_row(row);
        cx.notify();
    }

    /// A single-line input of row `row`'s card: typing redraws it (Send
    /// lights when the form is whole).
    /// Enter there, the card entered from the keys, goes forward
    /// ([`AppView::ask_field_enter`]).
    fn watch_input(row: u64, e: &Entity<InputState>, window: &mut Window, cx: &mut Context<Self>) -> Subscription {
        cx.subscribe_in(e, window, move |this: &mut Self, _, ev: &InputEvent, window, cx| match ev {
            InputEvent::Change => {
                this.remeasure_row(row);
                cx.notify();
            }
            InputEvent::PressEnter { .. } => this.ask_field_enter(row, window, cx),
            _ => {}
        })
    }

    /// Send on the card `painted`: its draft's answer, once whole.
    pub(crate) fn ask_submit(&mut self, row: u64, painted: &Painted, cx: &mut Context<Self>) {
        let Some(session) = self.card_session().filter(|_| self.ask_armed(row, painted, "send")) else { return };
        if self.card_submit(&session, row, painted, cx) {
            self.clicked_an_answer(cx);
        }
    }

    /// Send on the card `painted` of `session`, by a click or a key (D2b):
    /// its draft's answer, once whole (true: it went), else what it needs.
    pub(crate) fn card_submit(&mut self, session: &str, row: u64, painted: &Painted, cx: &mut Context<Self>) -> bool {
        let reply = self.draft(session, painted).reply(&painted.doc, cx);
        match reply {
            Ok(reply) => self.ask_send(session, row, painted, reply, cx),
            Err(why) => {
                self.draft(session, painted).said = Some(why);
                self.remeasure_row(row);
                cx.notify();
                false
            }
        }
    }

    /// Skip on the card `painted` (F7): `[card <id> skipped]`.
    pub(crate) fn ask_skip(&mut self, row: u64, painted: &Painted, cx: &mut Context<Self>) {
        let Some(session) = self.card_session().filter(|_| self.ask_armed(row, painted, "skip")) else { return };
        if self.ask_send(&session, row, painted, Reply::Skip, cx) {
            self.clicked_an_answer(cx);
        }
    }

    /// Press `reply` on the card `painted`: checked on what the store holds
    /// now ([`crate::model::Model::press_card`]), sent, its draft gone
    /// (true); or the card says why nothing went. D2b's keys send here
    /// too: the one delivery path.
    pub(crate) fn ask_send(&mut self, session: &str, row: u64, painted: &Painted, reply: Reply, cx: &mut Context<Self>) -> bool {
        let live = self.panes.cur.live;
        let sent = match self.model.press_card(live, painted, reply, Instant::now()) {
            Ok(out) => {
                self.send_card(live, out, cx);
                self.card_drafts.remove(&(session.to_string(), painted.tool_id.clone()));
                true
            }
            Err(why) => {
                log::info!(target: "synaps_desktop::card", "card {}: not sent ({why})", painted.id);
                self.draft(session, painted).said = Some(why);
                false
            }
        };
        self.after_card(row, cx);
        sent
    }

    /// Retry on the card `painted`'s answer, not sent.
    pub(crate) fn ask_retry(&mut self, row: u64, painted: &Painted, cx: &mut Context<Self>) {
        let Some(session) = self.card_session().filter(|_| self.ask_armed(row, painted, "retry")) else { return };
        let live = self.panes.cur.live;
        match self.model.retry_card(live, painted, Instant::now()) {
            Ok(out) => self.send_card(live, out, cx),
            Err(why) => self.draft(&session, painted).said = Some(why),
        }
        self.after_card(row, cx);
    }

    /// Resend on the card `painted`'s answer, unconfirmed: checked again on
    /// the click (§3.5).
    pub(crate) fn ask_resend(&mut self, row: u64, painted: &Painted, cx: &mut Context<Self>) {
        let Some(session) = self.card_session().filter(|_| self.ask_armed(row, painted, "resend")) else { return };
        let live = self.panes.cur.live;
        match self.model.resend_card(live, painted, Instant::now()) {
            Ok(out) => self.send_card(live, out, cx),
            Err(why) => self.draft(&session, painted).said = Some(why),
        }
        self.after_card(row, cx);
    }

    /// "Check again" on the card `painted`, dropped events not recovered
    /// from in time: the client asks for the history again, on this
    /// attachment, and the card checks for another while
    /// ([`crate::model::Model::check_again`]). No Resend meanwhile.
    pub(crate) fn ask_check_again(&mut self, row: u64, painted: &Painted, cx: &mut Context<Self>) {
        if self.card_session().filter(|_| self.ask_armed(row, painted, "check")).is_none() {
            return;
        }
        if let Some(fx) = self.model.check_again(self.panes.cur.live, Instant::now()) {
            self.effect(fx);
        }
        self.after_card(row, cx);
    }

    /// A receipt clicked: its whole answer, or back to its line.
    pub(crate) fn ask_open(&mut self, row: u64, tool_id: &str, cx: &mut Context<Self>) {
        let Some(session) = self.card_session() else { return };
        let key = (session, tool_id.to_string());
        if !self.card_open.remove(&key) {
            self.card_open.insert(key);
        }
        self.remeasure_row(row);
        cx.notify();
    }

    fn after_card(&mut self, row: u64, cx: &mut Context<Self>) {
        self.memo_seen = self.model.memos.changes();
        self.remeasure_row(row);
        self.arm_card_tick(cx);
        cx.notify();
    }

    /// Hand `out` to the client, on its attachment: a `Steer` while the
    /// session is busy, else a `Submit` (as the composer's). Its echo or
    /// refusal is its card's ([`crate::live::Target::Card`]).
    pub(crate) fn send_card(&mut self, live: crate::live::LiveId, out: Outgoing, cx: &mut Context<Self>) {
        #[cfg(test)]
        self.card_sends.push(out.clone());
        let Some(c) = self.client.as_ref() else {
            match self.synthetic_tx.clone().filter(|_| self.synthetic) {
                Some(tx) => self.synthetic_answer(live, out, tx, cx),
                None => self.model.card_unsent(live, "offline"),
            }
            return;
        };
        let sent = if out.busy { c.on(out.seq).steer(out.text.clone()) } else { c.on(out.seq).submit(out.text.clone()) };
        if sent.is_err() {
            self.model.card_unsent(live, "the connection thread has stopped");
            return;
        }
        log::debug!(target: "synaps_desktop::send", "card {}: {} ({} chars)", out.card, if out.busy { "steer" } else { "submit" }, out.text.chars().count());
        self.model.card_sent(live, out);
    }

    /// `--offline`: the synthetic daemon takes the answer `out` as a daemon
    /// would, so the demo's cards answer (sending, then answered): its echo
    /// a moment later, then the agent's line and the turn's end
    /// ([`crate::demo::card_answered`]).
    fn synthetic_answer(
        &mut self,
        live: crate::live::LiveId,
        out: Outgoing,
        tx: async_channel::Sender<synaps_client::Event>,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.model.lives[live].active.as_ref().map(|a| a.session_id.clone()) else { return };
        let (echo, reply) = crate::demo::card_answered(out.seq, &session, &out.text);
        self.model.card_sent(live, out);
        cx.spawn(async move |_, cx| {
            for (after, events) in [(crate::demo::ECHO_AFTER, echo), (crate::demo::REPLY_AFTER, reply)] {
                cx.background_executor().timer(after).await;
                for e in events {
                    if tx.send(e).await.is_err() {
                        return;
                    }
                }
            }
        })
        .detach();
    }

    /// The memos' clock while an answer waits on it (an echo, a settled
    /// view): a tick a second, none otherwise (cards at rest ask for
    /// nothing, §6.1).
    pub(crate) fn arm_card_tick(&mut self, cx: &mut Context<Self>) {
        if self.card_tick.is_some() || !self.model.memos.timed() {
            return;
        }
        self.card_tick = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_secs(1)).await;
            let _ = this.update(cx, |v, cx| {
                v.card_tick = None;
                v.model.tick_cards(Instant::now());
                v.cards_moved(cx);
                v.arm_card_tick(cx);
            });
        }));
    }

    /// The memos changed since the cards were last measured: every card row
    /// of every pane is measured again (a card becomes its receipt, a line
    /// comes or goes), and drawn.
    pub(crate) fn cards_moved(&mut self, cx: &mut Context<Self>) {
        let n = self.model.memos.changes();
        if n == self.memo_seen {
            return;
        }
        self.memo_seen = n;
        for pane in self.panes.iter_mut() {
            for (ix, d) in pane.disp.iter().enumerate() {
                if d.kind == Kind::Ask {
                    pane.list.remeasure_items(ix..ix + 1);
                }
            }
        }
        cx.notify();
    }

    /// Card row `row` is measured again, in every pane that shows it.
    pub(crate) fn remeasure_row(&mut self, row: u64) {
        for pane in self.panes.iter_mut() {
            if let Some(ix) = pane.disp.iter().position(|d| d.id == row && d.kind == Kind::Ask) {
                pane.list.remeasure_items(ix..ix + 1);
            }
        }
    }
}
