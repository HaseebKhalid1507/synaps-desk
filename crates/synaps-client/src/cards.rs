//! Agent cards in a transcript (synaps-cards `docs/design.md` v1.2, §2):
//! which rows draw a card, an answer to one or a close, and in what state.
//! Read-only: nothing here sends.
//!
//! The state is `cards_spec::fold`'s, the fold every desk runs (§2.3), over
//! the session's history and then the turn still streaming:
//! - **The history**, one message at a time as the transcript folds it,
//!   read as [`cards_spec::fold::events`] reads a whole history: every call
//!   (a card call's input whole), the card calls' results, and each user
//!   message that is the user's own words (the host's rule for its
//!   extension blocks). An update may start after a call whose result it
//!   holds, so the tool ids whose latest call is a card's are kept across
//!   updates ([`HistoryCards::calls`]); read so, any split of a history
//!   gives `events` of the whole.
//! - **The live turn**, from its rows, in the order their events came
//!   ([`crate::transcript::Item::seq()`]; results need not come in call
//!   order, and a card id bound twice names the card bound last): a call
//!   once its input is whole (`ToolUse`; a card call's input kept whole,
//!   §2.1), its result once it reports, and the user message that started
//!   a turn (`TurnStarted`), so an answer sent from another desk shows when
//!   its turn starts (F14), not when it ends. The history replaces them
//!   when it lands, as it replaces the rows.
//!
//! A card is drawn on its call's row ([`crate::transcript::Item::card`]):
//! a call that asks, bound or still waiting for its result. One whose input
//! doesn't validate or whose result doesn't bind is plain, `None`: an
//! ordinary tool row, the fail-safe (§2.2, §6.3). An answer the fold
//! counted is drawn on its message's row, and a close that closed a card on
//! its call's row: bound when its result came, for good (the fold names a
//! tool id's latest call only, and a later call of the id takes the id,
//! not the receipt).

use std::collections::HashMap;
use std::sync::Arc;

pub use cards_spec::fold::State;
use cards_spec::fold::{Event, Fold, NAMES, events};
pub use cards_spec::{Answer, CardDoc, CardField, CardId, CardOption, Choice, DefaultChoice, FieldKind, FieldValue, Mode, Reply};
use serde_json::Value;

use crate::transcript::{Item, ItemKind, ToolStatus};

/// What a row draws of the agent cards.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CardRow {
    /// A `cards_ui` call that asks: the card.
    Ask(Ask),
    /// A user message the fold counted as an answer to a card (§3.3): a
    /// slim row, not the message's two lines.
    Answer(Answered),
    /// A `cards_ui` call whose result closed a card.
    Close(Closed),
}

/// A card: what it asks, and where it stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ask {
    /// Its call's tool id: the card in [`crate::Transcript::cards`]
    /// (`Fold::card`), which an answer is checked against.
    pub tool_id: String,
    /// The normalized doc (`cards_spec::parse_input`), as the extension
    /// validated it.
    pub doc: Arc<CardDoc>,
    /// The call's mode: the agent can't go on without the answer.
    pub wait: bool,
    /// Its id, once its result bound it.
    pub id: Option<CardId>,
    /// Never [`State::Plain`]: a plain call draws no card.
    pub state: State,
    /// Its latest answer, kept once closed or replaced.
    pub reply: Option<Reply>,
    /// How many answers it had (`rev`, §2.3): 2 or more is a change.
    pub answers: usize,
}

/// An answer's message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answered {
    /// The card it answers: its doc says what the values mean.
    pub doc: Arc<CardDoc>,
    pub reply: Reply,
}

/// A close's call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Closed {
    /// The card it closed.
    pub doc: Arc<CardDoc>,
}

/// Whether `name` is a card call's (§2.2 step 1; a collision suffix,
/// `cards_ui_2`, is another tool).
pub fn is_card_tool(name: &str) -> bool {
    NAMES.contains(&name)
}

/// The fold over the history folded so far, read message by message (and,
/// the live turn read on top of it: [`LiveCards`]).
#[derive(Clone, Debug, Default)]
pub(crate) struct HistoryCards {
    /// Each event's row: a call's, a user message's (the row its text is
    /// drawn in); a result has none. Indexed as the fold numbers events.
    rows: Vec<Option<u64>>,
    /// Each tool id's latest call: whether it is a card call (the results
    /// `events` keeps are those), and its row.
    calls: HashMap<String, (bool, Option<u64>)>,
    /// Each close that closed a card, as its result came: its call's row
    /// and the card it closed.
    closes: Vec<(u64, Arc<CardDoc>)>,
    /// A card call read.
    called: bool,
    fold: Fold,
}

impl HistoryCards {
    /// No card call in the history.
    pub(crate) fn is_empty(&self) -> bool {
        !self.called
    }

    /// The fold of the history alone.
    pub(crate) fn fold(&self) -> &Fold {
        &self.fold
    }

    /// Read event `e`, drawn on `row` (a result's: none). `base`: the
    /// history this reads on top of, if it is a live layer
    /// ([`LiveCards::read`]): its latest calls are read under this one's,
    /// as a copy of it would read them (a live result's call is on the
    /// result's own row, read before it: the layer has it).
    fn push(&mut self, e: &Event, row: Option<u64>, base: Option<&HistoryCards>) {
        // The tool id of a close this result may close.
        let closing = match e {
            Event::Call { tool_id, name, .. } => {
                let card = is_card_tool(name);
                self.called |= card;
                self.calls.insert(tool_id.clone(), (card, row));
                None
            }
            Event::Result { tool_id, .. } => self.fold.close(tool_id).filter(|c| c.pending).map(|_| tool_id.as_str()),
            Event::User { .. } => None,
        };
        self.fold.apply(e);
        self.rows.push(row);
        // It closed a card: the one its card id names now. Kept on the
        // close's row whatever later calls its tool id.
        if let Some(tool_id) = closing
            && let Some(close) = self.fold.close(tool_id).filter(|c| c.closed)
            && let Some(doc) = self.fold.bound(&close.card).and_then(|c| c.doc.as_ref())
            && let Some(&(_, Some(r))) = self.calls.get(tool_id).or_else(|| base.and_then(|b| b.calls.get(tool_id)))
        {
            self.closes.push((r, Arc::new(doc.clone())));
        }
    }

    /// History message `m`, folded into `rows` (its rows, in order).
    pub(crate) fn message(&mut self, m: &Value, rows: &[Item]) {
        match m["role"].as_str() {
            Some("assistant") => {
                let mut tools = rows.iter().filter(|i| i.kind == ItemKind::Tool);
                for e in events(std::slice::from_ref(m)) {
                    let Event::Call { tool_id, .. } = &e else { continue };
                    let row = tools.by_ref().find(|i| i.tool.as_ref().is_some_and(|t| t.tool_id == *tool_id)).map(|i| i.id);
                    self.push(&e, row, None);
                }
            }
            Some("user") => {
                // The results `events` keeps: a card call's (its tool id's
                // latest call), which may be in an earlier update.
                for b in m["content"].as_array().into_iter().flatten().filter(|b| b["type"] == "tool_result") {
                    if let Some(id) = b["tool_use_id"].as_str().filter(|id| self.calls.get(*id).is_some_and(|c| c.0))
                        && let Some(text) = result_text(&b["content"])
                    {
                        self.push(&Event::Result { tool_id: id.to_string(), text }, None, None);
                    }
                }
                // Its text, if the host's rule leaves it one string: no
                // result in a one-message slice (its call is outside it).
                let row = rows.iter().find(|i| i.kind == ItemKind::User).map(|i| i.id);
                for e in events(std::slice::from_ref(m)).iter().filter(|e| matches!(e, Event::User { .. })) {
                    self.push(e, row, None);
                }
            }
            _ => {}
        }
    }
}

/// A `tool_result`'s text as `cards_spec::fold::events` reads it: its
/// string, or its text blocks joined with nothing between. (The
/// transcript's own joins them with a newline: a result split mid-line
/// would bind here and not there.)
fn result_text(content: &Value) -> Option<String> {
    match content {
        Value::String(s) => Some(s.clone()),
        Value::Array(blocks) => {
            let texts: Vec<&str> = blocks.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect();
            (!texts.is_empty()).then(|| texts.concat())
        }
        _ => None,
    }
}

/// A live row's event, borrowed from the row: the event is built for the
/// fold to read ([`RowEvent::event`]), and the sync keeps its digest only.
enum RowEvent<'a> {
    /// A non-card call's input is `Null` to the fold (`input`: none).
    Call {
        tool_id: &'a str,
        name: &'a str,
        input: Option<&'a Value>,
    },
    Result {
        tool_id: &'a str,
        text: &'a str,
    },
    User {
        text: &'a str,
    },
}

impl RowEvent<'_> {
    /// The event, for the fold to read (a copy, dropped once read: of a
    /// card call's input the fold keeps the doc it validates, if any).
    fn event(&self) -> Event {
        match *self {
            RowEvent::Call { tool_id, name, input } => {
                Event::Call { tool_id: tool_id.to_string(), name: name.to_string(), input: input.cloned().unwrap_or(Value::Null) }
            }
            RowEvent::Result { tool_id, text } => Event::Result { tool_id: tool_id.to_string(), text: text.to_string() },
            RowEvent::User { text } => Event::User { text: text.to_string() },
        }
    }

    /// What [`LiveCards::seen`] keeps of it to tell whether it changed
    /// since: a digest of all of it (an input as its JSON), not a copy.
    /// Two events of a row at one place that differ but digest alike
    /// would go unnoticed: 64 bits of SipHash make that a 2⁻⁶⁴ chance an
    /// event changed in place, itself rare (a result's text streaming on
    /// after it reported).
    fn digest(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        struct Write<'h>(&'h mut std::collections::hash_map::DefaultHasher);
        impl std::io::Write for Write<'_> {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.write(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut h = std::collections::hash_map::DefaultHasher::new();
        match *self {
            RowEvent::Call { tool_id, name, input } => {
                (0u8, tool_id, name).hash(&mut h);
                // Writing to a hasher can't fail.
                let _ = serde_json::to_writer(Write(&mut h), input.unwrap_or(&Value::Null));
            }
            RowEvent::Result { tool_id, text } => (1u8, tool_id, text).hash(&mut h),
            RowEvent::User { text } => (2u8, text).hash(&mut h),
        }
        h.finish()
    }

    /// The row it is drawn on, of row `id`'s: a result's, none.
    fn drawn_on(&self, id: u64) -> Option<u64> {
        (!matches!(self, RowEvent::Result { .. })).then_some(id)
    }
}

/// A live row's events, each with its place in the stream: a call once
/// its input is whole (the history never holds a call that is still
/// streaming), a card call's result (no other binds: it is nothing to the
/// fold), the user message that started a turn.
fn row_events(it: &Item) -> Vec<(u64, RowEvent<'_>)> {
    let mut out = Vec::new();
    if it.is_note() {
        return out;
    }
    let [came, reported] = it.seq();
    match it.kind {
        ItemKind::Tool => {
            let Some(t) = it.tool.as_ref().filter(|t| !t.tool_id.is_empty() && t.status != Some(ToolStatus::Streaming)) else {
                return out;
            };
            let card = is_card_tool(&t.name);
            let input = if card { it.card_input() } else { None };
            out.push((came, RowEvent::Call { tool_id: &t.tool_id, name: &t.name, input }));
            if card && t.status.is_some_and(ToolStatus::is_terminal) {
                out.push((reported, RowEvent::Result { tool_id: &t.tool_id, text: &t.result }));
            }
        }
        ItemKind::User => out.push((came, RowEvent::User { text: &it.text })),
        _ => {}
    }
    out
}

/// The bytes of `m`'s table (hashbrown's: a slot and a control byte per
/// bucket, a group of control bytes more), its keys' and values' own heap
/// not included.
pub(crate) fn map_bytes<K, V>(m: &HashMap<K, V>) -> usize {
    let cap = m.capacity();
    if cap == 0 {
        return 0;
    }
    let buckets = if cap < 8 { (cap + 1).next_power_of_two() } else { (cap * 8 / 7).next_power_of_two() };
    buckets * (std::mem::size_of::<(K, V)>() + 1) + 16
}

/// The cards the rows draw: the history's fold, then the live rows read on
/// top, in the order their events came. Kept between syncs, it reads only
/// the rows that changed ([`LiveCards::step`]).
///
/// What it keeps beyond the fold (main's `card_fold`) is the sync's, to
/// read what changed: a digest per live event, the live events' rows and
/// latest calls, and an id and a place per row drawn. No event's input or
/// text: those are the rows'. [`LiveCards::bytes`] counts it, and
/// [`LiveCards::unkeep`] lets it go (the next sync reads every row).
#[derive(Clone, Debug, Default)]
pub(crate) struct LiveCards {
    /// The fold (the history's, then the live events read on top); the
    /// rows, latest calls and closes of the live events alone (the
    /// history's are its own: [`HistoryCards::push`]'s `base`).
    read: HistoryCards,
    /// The history's events, as read: a history that moved since is not
    /// stepped on ([`LiveCards::step`]).
    base: usize,
    /// Each live row's events read, by row id: where each came, and its
    /// digest ([`RowEvent::digest`]).
    seen: HashMap<u64, Vec<(u64, u64)>>,
    /// The place of the last live event read (0: none).
    last: u64,
    /// What each row draws, by row id: the row's own ([`Item::card`]), not
    /// a copy.
    pub(crate) want: HashMap<u64, Arc<CardRow>>,
    /// Where each row that draws a card was when it was drawn (it may have
    /// moved since: the caller checks).
    pub(crate) at: HashMap<u64, usize>,
}

/// What a [`LiveCards::step`] changed.
pub(crate) struct Step {
    /// The fold's cards.
    pub(crate) cards: bool,
    /// The rows whose card changed, by id.
    pub(crate) rows: Vec<u64>,
}

impl LiveCards {
    /// The history's fold, then the live rows (`live`, the rows after the
    /// history's).
    pub(crate) fn of(history: &HistoryCards, live: &[Item]) -> Self {
        let mut events: Vec<(u64, Event, Option<u64>)> = Vec::new();
        let mut seen = HashMap::new();
        for it in live {
            let e = row_events(it);
            if !e.is_empty() {
                events.extend(e.iter().map(|(at, e)| (*at, e.event(), e.drawn_on(it.id))));
                seen.insert(it.id, e.iter().map(|(at, e)| (*at, e.digest())).collect());
            }
        }
        events.sort_by_key(|(at, ..)| *at);
        let mut read = HistoryCards { fold: history.fold.clone(), ..HistoryCards::default() };
        for (_, e, row) in &events {
            read.push(e, *row, Some(history));
        }
        let last = events.last().map_or(0, |(at, ..)| *at);
        let want = want(history, &read).into_iter().map(|(id, c)| (id, Arc::new(c))).collect();
        LiveCards { read, base: history.rows.len(), seen, last, want, at: HashMap::new() }
    }

    /// The fold the rows draw.
    pub(crate) fn fold(&self) -> &Fold {
        &self.read.fold
    }

    /// Let go of what reads what changed, the fold kept: until the next
    /// [`LiveCards::of`], [`LiveCards::step`] must not be called.
    pub(crate) fn unkeep(&mut self) {
        let fold = std::mem::take(&mut self.read.fold);
        *self = LiveCards { read: HistoryCards { fold, ..HistoryCards::default() }, ..LiveCards::default() };
    }

    /// The heap it keeps beyond the fold, capacities counted: what
    /// [`LiveCards::unkeep`] lets go. (A row's card is the row's: counted
    /// as a pointer.)
    pub(crate) fn bytes(&self) -> usize {
        let r = &self.read;
        map_bytes(&self.seen)
            + self.seen.values().map(|v| v.capacity() * std::mem::size_of::<(u64, u64)>()).sum::<usize>()
            + r.rows.capacity() * std::mem::size_of::<Option<u64>>()
            + map_bytes(&r.calls)
            + r.calls.keys().map(String::capacity).sum::<usize>()
            + r.closes.capacity() * std::mem::size_of::<(u64, Arc<CardDoc>)>()
            + map_bytes(&self.want)
            + map_bytes(&self.at)
    }

    /// Read what `rows` (live rows that may have changed since) have new:
    /// events that came after every one read, read on top of `history`
    /// (the one it was read on). `None` if a row changed otherwise (an
    /// event read is gone or different, or a new one came before one
    /// read), or the history did: read them all again ([`LiveCards::of`]).
    pub(crate) fn step<'a>(&mut self, history: &HistoryCards, rows: impl IntoIterator<Item = &'a Item>) -> Option<Step> {
        if history.rows.len() != self.base {
            return None;
        }
        let mut new: Vec<(u64, Event, Option<u64>)> = Vec::new();
        for it in rows {
            let now = row_events(it);
            let had = self.seen.get(&it.id).map_or(&[][..], Vec::as_slice);
            if now.len() < had.len() || had.iter().zip(&now).any(|(&(was, d), (at, e))| was != *at || d != e.digest()) {
                return None;
            }
            for (at, e) in &now[had.len()..] {
                if *at <= self.last {
                    return None;
                }
                new.push((*at, e.event(), e.drawn_on(it.id)));
            }
            if now.len() > had.len() {
                let mut d: Vec<(u64, u64)> = had.to_vec();
                d.extend(now[had.len()..].iter().map(|(at, e)| (*at, e.digest())));
                self.seen.insert(it.id, d);
            }
        }
        new.sort_by_key(|(at, ..)| *at);
        // Another tool's call changes no card (it takes its tool id over:
        // a card still pending there turns plain, the one case it does).
        let mut before = None;
        for (at, e, row) in &new {
            let inert = match e {
                Event::Call { tool_id, name, .. } => {
                    !is_card_tool(name) && !self.read.fold.card(tool_id).is_some_and(|c| c.state == State::Pending)
                }
                _ => false,
            };
            if !inert && before.is_none() {
                before = Some(self.read.fold.cards().to_vec());
            }
            self.read.push(e, *row, Some(history));
            self.last = *at;
        }
        drop(new);
        let Some(before) = before else {
            return Some(Step { cards: false, rows: Vec::new() });
        };
        // A row whose card is the same keeps it (the row holds it too).
        let mut rows = Vec::new();
        let mut next = HashMap::new();
        for (id, c) in want(history, &self.read) {
            let c = match self.want.remove(&id) {
                Some(was) if *was == c => was,
                _ => {
                    rows.push(id);
                    Arc::new(c)
                }
            };
            next.insert(id, c);
        }
        rows.extend(self.want.keys());
        self.want = next;
        Some(Step { cards: before != self.read.fold.cards(), rows })
    }
}

/// What each row draws of `read`'s fold, by row id: `read` a live layer
/// on `history` ([`LiveCards::read`]), its events numbered after the
/// history's.
fn want(history: &HistoryCards, read: &HistoryCards) -> HashMap<u64, CardRow> {
    let base = history.rows.len();
    let row = |at: usize| if at < base { history.rows.get(at) } else { read.rows.get(at - base) }.copied().flatten();
    let mut out = HashMap::new();
    for c in read.fold.cards() {
        let (Some(doc), Some(r)) = (c.doc.as_ref().filter(|_| c.state != State::Plain), row(c.at)) else { continue };
        let doc = Arc::new(doc.clone());
        let ask = Ask {
            tool_id: c.tool_id.clone(),
            doc: doc.clone(),
            wait: c.wait,
            id: c.id.clone(),
            state: c.state,
            reply: c.receipt().map(|r| r.reply.clone()),
            answers: c.answers.len(),
        };
        out.insert(r, CardRow::Ask(ask));
        for a in &c.answers {
            if let Some(r) = row(a.at) {
                out.insert(r, CardRow::Answer(Answered { doc: doc.clone(), reply: a.reply.clone() }));
            }
        }
    }
    for (r, doc) in history.closes.iter().chain(&read.closes) {
        out.insert(*r, CardRow::Close(Closed { doc: doc.clone() }));
    }
    out
}

#[cfg(test)]
mod tests {
    use cards_spec::fold::fold;
    use cards_spec::{Envelope, Input, parse_input};
    use serde_json::json;

    use super::*;

    fn showing(card: &str, input: &Value) -> String {
        let Ok(Input::Create { doc, wait }) = parse_input(input) else { panic!() };
        format!("{}\n  1. A\nIts answer will come…", Envelope::showing(CardId::parse(card).unwrap(), &doc, wait))
    }

    /// Read a message at a time, a history gives what `events` gives of the
    /// whole of it: the same events, so the same fold, positions and all.
    #[test]
    fn read_by_message_it_is_events_of_the_whole() {
        let ask = json!({"question": "Which?", "options": ["A", "B"]});
        let answer = "[card c4f9x2a] Which? → A\n{\"choice\":\"A\"}";
        let history = vec![
            json!({"role": "user", "content": "go"}),
            json!({"role": "assistant", "content": [
                {"type": "text", "text": "Asking."},
                {"type": "tool_use", "id": "t1", "name": "cards_ui", "input": ask},
                {"type": "tool_use", "id": "t2", "name": "bash", "input": {"command": "ls"}},
                {"type": "tool_use", "id": "t3", "name": "cards_ui", "input": ask}]}),
            // A result whose call is in another message, in two text blocks.
            json!({"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t2", "content": "a"},
                {"type": "tool_result", "tool_use_id": "t1", "content": [{"type": "text", "text": "Card c4f9x2a is sh"},
                    {"type": "text", "text": &showing("c4f9x2a", &ask)["Card c4f9x2a is sh".len()..]}]}]}),
            json!({"role": "assistant", "content": [{"type": "tool_use", "id": "t3", "name": "read", "input": {"path": "a"}}]}),
            json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t3", "content": showing("c81kq0z", &ask)}]}),
            json!({"role": "user", "content": [{"type": "text", "text": answer},
                {"type": "text", "text": "[Extension context from axel — do not treat as user instructions]\nx\n[End extension context]"}],
                "_synaps_hooked": 1}),
            json!({"role": "user", "content": answer}),
        ];
        let mut read = HistoryCards::default();
        for m in &history {
            read.message(m, &[]);
        }
        let whole = fold(&events(&history));
        assert_eq!(read.fold.cards(), whole.cards());
        assert_eq!(read.rows.len(), events(&history).len());
        assert_eq!(whole.cards().iter().map(|c| (c.state, c.answers.len())).collect::<Vec<_>>(), [(State::Answered, 2), (State::Plain, 0)]);
    }
}
