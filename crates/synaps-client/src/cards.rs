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
//! - **The live turn**, from its rows: a call once its input is whole
//!   (`ToolUse`; a card call's input kept whole, §2.1), its result once it
//!   reports, and the user message that started a turn (`TurnStarted`), so
//!   an answer sent from another desk shows when its turn starts (F14),
//!   not when it ends. The history replaces them when it lands, as it
//!   replaces the rows.
//!
//! A card is drawn on its call's row ([`crate::transcript::Item::card`]):
//! a call that asks, bound or still waiting for its result. One whose input
//! doesn't validate or whose result doesn't bind is plain, `None`: an
//! ordinary tool row, the fail-safe (§2.2, §6.3). An answer the fold
//! counted is drawn on its message's row, and a close that closed a card on
//! its call's row.

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

/// The fold over the history folded so far, read message by message.
#[derive(Clone, Debug, Default)]
pub(crate) struct HistoryCards {
    /// Each event's row: a call's, a user message's (the row its text is
    /// drawn in); a result has none. Indexed as the fold numbers events.
    rows: Vec<Option<u64>>,
    /// Each tool id's latest call: whether it is a card call (the results
    /// `events` keeps are those).
    calls: HashMap<String, bool>,
    /// The card calls, in order: tool id, event, row (a close is drawn on
    /// its row).
    card_calls: Vec<(String, usize, Option<u64>)>,
    fold: Fold,
}

impl HistoryCards {
    /// No card call in the history.
    pub(crate) fn is_empty(&self) -> bool {
        self.card_calls.is_empty()
    }

    fn push(&mut self, e: &Event, row: Option<u64>) {
        if let Event::Call { tool_id, name, .. } = e {
            let card = is_card_tool(name);
            self.calls.insert(tool_id.clone(), card);
            if card {
                self.card_calls.push((tool_id.clone(), self.rows.len(), row));
            }
        }
        self.fold.apply(e);
        self.rows.push(row);
    }

    /// History message `m`, folded into `rows` (its rows, in order).
    pub(crate) fn message(&mut self, m: &Value, rows: &[Item]) {
        match m["role"].as_str() {
            Some("assistant") => {
                let mut tools = rows.iter().filter(|i| i.kind == ItemKind::Tool);
                for e in events(std::slice::from_ref(m)) {
                    let Event::Call { tool_id, .. } = &e else { continue };
                    let row = tools.by_ref().find(|i| i.tool.as_ref().is_some_and(|t| t.tool_id == *tool_id)).map(|i| i.id);
                    self.push(&e, row);
                }
            }
            Some("user") => {
                // The results `events` keeps: a card call's (its tool id's
                // latest call), which may be in an earlier update.
                for b in m["content"].as_array().into_iter().flatten().filter(|b| b["type"] == "tool_result") {
                    if let Some(id) = b["tool_use_id"].as_str().filter(|id| self.calls.get(*id) == Some(&true))
                        && let Some(text) = result_text(&b["content"])
                    {
                        self.push(&Event::Result { tool_id: id.to_string(), text }, None);
                    }
                }
                // Its text, if the host's rule leaves it one string: no
                // result in a one-message slice (its call is outside it).
                let row = rows.iter().find(|i| i.kind == ItemKind::User).map(|i| i.id);
                for e in events(std::slice::from_ref(m)).iter().filter(|e| matches!(e, Event::User { .. })) {
                    self.push(e, row);
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

/// What each row draws: the history's fold, then the live rows
/// (`live`, the rows after the history's) read on top. By row id.
pub(crate) fn rows(history: &HistoryCards, live: &[Item]) -> HashMap<u64, CardRow> {
    let mut fold = history.fold.clone();
    // The live events' rows, numbered on from the history's.
    let base = history.rows.len();
    let mut live_rows: Vec<Option<u64>> = Vec::new();
    let mut card_calls: Vec<(&str, usize, Option<u64>)> = Vec::new();
    for it in live.iter().filter(|i| !i.is_note()) {
        match it.kind {
            ItemKind::Tool => {
                // A call is one once its input is whole: the history never
                // holds a call that is still streaming.
                let Some(t) = it.tool.as_ref().filter(|t| !t.tool_id.is_empty() && t.status != Some(ToolStatus::Streaming)) else {
                    continue;
                };
                let card = is_card_tool(&t.name);
                let input = if card { it.card_input().cloned().unwrap_or(Value::Null) } else { Value::Null };
                if card {
                    card_calls.push((&t.tool_id, base + live_rows.len(), Some(it.id)));
                }
                fold.apply(&Event::Call { tool_id: t.tool_id.clone(), name: t.name.clone(), input });
                live_rows.push(Some(it.id));
                // Only a card call's result can bind; another's is nothing
                // to the fold.
                if card && t.status.is_some_and(ToolStatus::is_terminal) {
                    fold.apply(&Event::Result { tool_id: t.tool_id.clone(), text: t.result.clone() });
                    live_rows.push(None);
                }
            }
            ItemKind::User => {
                fold.apply(&Event::User { text: it.text.clone() });
                live_rows.push(Some(it.id));
            }
            _ => {}
        }
    }
    let row = |at: usize| if at < base { history.rows.get(at).copied().flatten() } else { live_rows.get(at - base).copied().flatten() };
    let mut out = HashMap::new();
    for c in fold.cards() {
        let (Some(doc), Some(r)) = (c.doc.as_ref().filter(|_| c.state != State::Plain), row(c.at)) else { continue };
        let doc = Arc::new(doc.clone());
        let ask = Ask {
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
    // A close is drawn on its tool id's latest card call, if that call is
    // still the id's latest ([`Fold::close`]) and closed a card: the last
    // card bound to the id before it.
    let mut latest: HashMap<&str, (usize, Option<u64>)> = HashMap::new();
    for (tool_id, at, r) in history.card_calls.iter().map(|(t, at, r)| (t.as_str(), *at, *r)).chain(card_calls) {
        latest.insert(tool_id, (at, r));
    }
    for (tool_id, (at, r)) in latest {
        let (Some(r), Some(close)) = (r, fold.close(tool_id).filter(|c| c.closed)) else { continue };
        let card = fold.cards().iter().rev().find(|c| c.at < at && c.id.as_ref() == Some(&close.card) && c.state == State::Closed);
        if let Some(doc) = card.and_then(|c| c.doc.as_ref()) {
            out.insert(r, CardRow::Close(Closed { doc: Arc::new(doc.clone()) }));
        }
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
