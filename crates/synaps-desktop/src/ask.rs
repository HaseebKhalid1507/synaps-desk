//! Agent cards (synaps-cards `docs/design.md` v1.2, §6.1; the D1 row of
//! §9.3): a `cards_ui` call drawn as the question it asks, read-only.
//! Which rows are cards, and in what state, is `synaps_client::cards`'
//! fold (the same over the history and the live turn); this draws it.
//!
//! In the Cards D language (square, `card_d`, no outline: Haseeb), a
//! message of the agent's own: never inside a run, so no fold hides one.
//! - **Open**, the whole card: its head (a mark and `QUESTION`, or
//!   `NEEDS YOU` in the accent when the agent waits on it), the question,
//!   the detail, the options with their numbers (the result numbers them
//!   so for the agent: "2" typed is the second) and the agent's default
//!   marked `suggested`, the fields with their prefills, and a footer that
//!   says how to answer. Asked but not bound yet (its result still to
//!   come, milliseconds), the same, `asking…`.
//! - **Answered, skipped, closed, replaced**: its receipt, one line, the
//!   card's head line: the question → what was answered, and the state
//!   word at its end (`answered`, `changed`, `skipped`, `closed`,
//!   `replaced`).
//! - **An answer's message** is the user's turn, one slim line on the
//!   user's surface: `↳ answered «Which approach?» → Refactor the parser`,
//!   never its JSON.
//!
//! Every string a card draws goes through the spec's sanitizer first
//! ([`shown`], §5.2): an answer's values are validated, never rewritten,
//! so one may hold a bidi control, a tab or an invisible.
//!
//! Nothing here takes a click or a key (D2a and D2b answer). A card is a
//! static row: no frame is asked for, at rest or otherwise.

use cards_spec::Lines;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use synaps_client::cards::{Answer, Ask, CardDoc, CardRow, Choice, DefaultChoice, FieldKind, FieldValue, Mode, Reply, State};
use synaps_client::transcript::{Item, ItemKind};

use crate::app::{AppView, display};
use crate::fonts::{self, TextPx as _};
use crate::glyph::{self, Glyph};
use crate::theme::{Metrics, Palette};

/// A `cards_ui` call that draws a card ([`crate::turns::Kind::Ask`]).
pub(crate) fn asks(it: &Item) -> bool {
    it.kind == ItemKind::Tool && matches!(it.card.as_deref(), Some(CardRow::Ask(_)))
}

/// A user message that answers one ([`crate::turns::Kind::Answer`]).
pub(crate) fn answers(it: &Item) -> bool {
    it.kind == ItemKind::User && matches!(it.card.as_deref(), Some(CardRow::Answer(_)))
}

/// `s` as a card draws it: `cards_spec::ui_sanitize`, the display side of
/// the one sanitizer (synaps-cards §5.2). A doc's strings were normalized
/// by it, so it changes none of them (it is idempotent); an answer's values
/// were validated, never rewritten (the reply keeps them as sent).
pub(crate) fn shown(s: &str, lines: Lines) -> String {
    cards_spec::ui_sanitize(s, lines)
}

/// A question as a one-line label ([`shown`]), cut to the spec's bound:
/// that of a call this desk can't read (§6.3) was never validated.
pub(crate) fn question_line(q: &str) -> String {
    cards_spec::sanitize::cut_chars(shown(q, Lines::Single).trim(), cards_spec::QUESTION_MAX)
}

/// A receipt's or an answer's line: the question takes up to this much of
/// it, the answer the rest.
const QUESTION_SHARE: f32 = 0.55;

/// A receipt's question: with an answer after it, whole up to
/// [`QUESTION_SHARE`] of the line; alone, as wide as the line.
fn question_box(d: Div, answered: bool) -> Div {
    if answered { d.flex_none().max_w(relative(QUESTION_SHARE)) } else { d.min_w_0().flex_shrink(1.) }
}

/// The card's padding: its head line sits this far under its top.
fn pad_y(m: &Metrics) -> f32 {
    (12. * m.r).round()
}

fn pad_x(m: &Metrics) -> f32 {
    (14. * m.r).round()
}

/// A text's whole-pixel line (as the authorize card's: a half pixel puts
/// every row under it off the grid).
fn line(size: f32) -> Pixels {
    px((size * 1.5).round())
}

/// The head line's height: the card's first line, and all of a receipt.
fn head_h(m: &Metrics) -> f32 {
    (m.label * 1.5).round().max(crate::glyph::Glyph::Needs.size() + 4.)
}

/// The `AGENT` label's top in its row: level with the head line.
pub(crate) fn label_top(m: &Metrics) -> f32 {
    (pad_y(m) + (head_h(m) - m.body_line) / 2.).max(0.)
}

/// Whether the card is drawn whole (else its receipt).
pub(crate) fn whole(a: &Ask) -> bool {
    matches!(a.state, State::Pending | State::Open)
}

/// The state, as the card's head or its receipt says it.
pub(crate) fn state_word(a: &Ask) -> &'static str {
    match a.state {
        State::Pending => "asking…",
        State::Open if a.wait => "the agent is waiting",
        State::Open => "open",
        State::Answered if a.answers > 1 => "changed",
        State::Answered => "answered",
        State::Skipped => "skipped",
        State::Closed => "closed",
        State::Replaced => "replaced",
        State::Plain => "",
    }
}

/// How to answer it, while no click or key does (D1): in the composer, as
/// the agent's result told it to read a reply.
pub(crate) fn how(doc: &CardDoc) -> &'static str {
    match (doc.mode(), doc.multi, doc.options.is_empty()) {
        (Mode::Quick, _, _) => "Reply with its number, or in your own words",
        (Mode::Form, true, false) => "Reply with the numbers you pick, or in your own words",
        (Mode::Form, _, _) => "Reply in your own words",
    }
}

/// What an answer says, in a line: the labels picked (`(none)` for an
/// empty pick), the note, then `Label: value` for each field; `None` for a
/// skip. [`shown`] on one line: a value's line breaks and tabs are spaces,
/// its controls and invisibles gone.
pub(crate) fn summary(doc: &CardDoc, reply: &Reply) -> Option<String> {
    let a: &Answer = match reply {
        Reply::Answer(a) | Reply::Change(a) => a,
        Reply::Skip => return None,
    };
    let mut parts: Vec<String> = Vec::new();
    match &a.choice {
        Some(Choice::One(l)) => parts.push(l.clone()),
        Some(Choice::Many(ls)) if ls.is_empty() => parts.push("(none)".into()),
        Some(Choice::Many(ls)) => parts.push(ls.join(", ")),
        None => {}
    }
    if let Some(note) = &a.note {
        parts.push(format!("“{note}”"));
    }
    for (id, v) in a.fields.iter().flatten() {
        let label = doc.field(id).map_or(id.as_str(), |f| f.label.as_str());
        let value = match v {
            FieldValue::Check(true) => "yes",
            FieldValue::Check(false) => "no",
            FieldValue::Text(t) if t.is_empty() => "—",
            FieldValue::Text(t) => t.as_str(),
        };
        parts.push(format!("{label}: {value}"));
    }
    Some(shown(&parts.join(" · "), Lines::Single))
}

/// Whether `label` is (one of) the agent's default.
fn suggested(doc: &CardDoc, label: &str) -> bool {
    match &doc.default {
        Some(DefaultChoice::One(l)) => l == label,
        Some(DefaultChoice::Many(ls)) => ls.iter().any(|l| l == label),
        None => false,
    }
}

/// The receipt's mark: what became of it.
fn receipt_mark(a: &Ask, p: &Palette) -> (Glyph, Hsla) {
    match a.state {
        State::Answered => (Glyph::Ok, p.ok),
        State::Skipped => (Glyph::Cancelled, p.dim_t),
        _ => (Glyph::Gone, p.dim_t),
    }
}

/// The card, open (or asking): head, question, detail, options, fields,
/// footer.
pub(crate) fn card(a: &Ask, id: u64, p: &Palette, m: &Metrics) -> Div {
    let doc = &a.doc;
    let r = m.r;
    let open = a.state == State::Open;
    let word = state_word(a);
    let mark: AnyElement = if a.wait && open {
        div().debug_selector(move || format!("ask-needs {id}")).child(glyph::draw(Glyph::Needs, p.acc)).into_any_element()
    } else {
        div().size(px(6.)).rounded(px(3.)).bg(if open { p.acc } else { p.dim_t }).into_any_element()
    };
    let (head_word, word_color) = if a.wait && open { (fonts::label::NEEDS_YOU, p.acc) } else { (fonts::label::QUESTION, p.dim_t) };
    let head = h_flex()
        .h(px(head_h(m)))
        .gap(px(8.))
        .items_center()
        .child(div().flex_none().size(px(Glyph::Needs.size())).flex().items_center().justify_center().child(mark))
        .child(display(head_word, fonts::size::D_S, word_color).debug_selector(move || format!("ask-word {id}")))
        .child(div().flex_1())
        .child(
            div()
                .flex_none()
                .font_family(fonts::SANS)
                .text_px(m.meta)
                .line_height(line(m.meta))
                .text_color(if a.wait && open { p.acc } else { p.dim_t })
                .whitespace_nowrap()
                .debug_selector(move || format!("ask-state {id} {word}"))
                .child(word),
        );
    let question = div()
        .mt(px((6. * r).round()))
        .font_family(fonts::SANS)
        .font_weight(FontWeight::SEMIBOLD)
        .text_px(m.card_lead)
        .line_height(px(m.card_lead_line))
        .text_color(p.ink)
        .debug_selector(move || format!("ask-question {id}"))
        .child(shown(&doc.question, Lines::Single));
    let detail = doc.detail.as_ref().map(|d| {
        let d = shown(d, Lines::Multi);
        v_flex()
            .mt(px((4. * r).round()))
            .font_family(fonts::SANS)
            .text_px(m.label)
            .line_height(line(m.label))
            .text_color(p.prose_t)
            .debug_selector(move || format!("ask-detail {id}"))
            .children(d.split('\n').map(|l| div().min_h(line(m.label)).child(l.to_string())))
    });
    let key = (20. * r).round();
    let options = (!doc.options.is_empty()).then(|| {
        v_flex().mt(px((10. * r).round())).gap(px((2. * r).round())).children(doc.options.iter().enumerate().map(|(i, o)| {
            let n = i + 1;
            let hint = o.hint.as_ref().map(|h| {
                div()
                    .min_w_0()
                    .flex_shrink(1.)
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_px(m.meta)
                    .text_color(p.dim_t)
                    .child(shown(h, Lines::Single))
            });
            let note = o.note.as_ref().map(|_| div().flex_none().text_px(m.meta).text_color(p.dim_t).child("+ a note"));
            h_flex()
                .min_h(px((30. * r).round()))
                .gap(px((10. * r).round()))
                .items_center()
                .font_family(fonts::SANS)
                .debug_selector(move || format!("ask-option {id} {n}"))
                .child(
                    div()
                        .flex_none()
                        .size(px(key))
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(p.well_d)
                        .font_family(fonts::MONO)
                        .text_px(m.meta)
                        .text_color(p.dim_t)
                        .child(n.to_string()),
                )
                .child(
                    div()
                        .flex_none()
                        .max_w(px((360. * r).round()))
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_px(m.card_title)
                        .text_color(p.prose_t)
                        .child(shown(&o.label, Lines::Single)),
                )
                .children(hint)
                .children(note)
                .child(div().flex_1())
                .when(suggested(doc, &o.label), |d| {
                    d.child(
                        div()
                            .flex_none()
                            .text_px(m.meta)
                            .text_color(p.acc)
                            .debug_selector(move || format!("ask-suggested {id} {n}"))
                            .child("suggested"),
                    )
                })
        }))
    });
    let fields = (!doc.fields.is_empty()).then(|| {
        v_flex().mt(px((10. * r).round())).gap(px((8. * r).round())).children(doc.fields.iter().map(|f| {
            let fid = f.id.clone();
            let value: AnyElement = match (&f.kind, &f.value) {
                (FieldKind::Check, v) => {
                    let on = *v == FieldValue::Check(true);
                    h_flex()
                        .gap(px(8.))
                        .items_center()
                        .child(
                            div()
                                .flex_none()
                                .size(px(12.))
                                .border_1()
                                .border_color(p.dim_t)
                                .flex()
                                .items_center()
                                .justify_center()
                                .when(on, |d| d.child(div().size(px(6.)).bg(p.acc))),
                        )
                        .child(div().text_px(m.card_title).text_color(p.prose_t).child(shown(&f.label, Lines::Single)))
                        .into_any_element()
                }
                (_, FieldValue::Text(t)) => {
                    let (text, color) = match (t.is_empty(), &f.placeholder) {
                        (false, _) => (shown(t, f.kind.lines()), p.ink),
                        (true, Some(ph)) => (shown(ph, Lines::Single), p.dim_t),
                        (true, None) => (String::new(), p.dim_t),
                    };
                    v_flex()
                        .gap(px((4. * r).round()))
                        .child(div().text_px(m.meta).text_color(p.dim_t).child(shown(&f.label, Lines::Single)))
                        .child(
                            div()
                                .w_full()
                                .min_h(px((28. * r).round()))
                                .px(px((8. * r).round()))
                                .py(px((4. * r).round()))
                                .bg(p.well_d)
                                .font_family(fonts::MONO)
                                .text_px(m.mono)
                                .line_height(px(m.mono_line))
                                .text_color(color)
                                .when(f.kind == FieldKind::Long, |d| d.min_h(px(3. * m.mono_line + (8. * r).round())))
                                .children(text.split('\n').map(|l| div().min_h(px(m.mono_line)).child(l.to_string()))),
                        )
                        .into_any_element()
                }
                (_, FieldValue::Check(_)) => div().into_any_element(),
            };
            div().font_family(fonts::SANS).debug_selector(move || format!("ask-field {id} {fid}")).child(value)
        }))
    });
    let foot = (a.state != State::Pending).then(|| how(doc));
    let foot = foot.map(|f| {
        div()
            .mt(px((10. * r).round()))
            .font_family(fonts::SANS)
            .text_px(m.meta)
            .line_height(line(m.meta))
            .text_color(p.dim_t)
            .debug_selector(move || format!("ask-footer {id}"))
            .child(f)
    });
    v_flex()
        .w_full()
        .bg(p.card_d)
        .px(px(pad_x(m)))
        .pt(px(pad_y(m)))
        .pb(px(pad_y(m)))
        .debug_selector(move || format!("ask-card {id}"))
        .child(head)
        .child(question)
        .children(detail)
        .children(options)
        .children(fields)
        .children(foot)
}

/// The card's receipt: its head line, the question → the answer, the state
/// word at its end.
pub(crate) fn receipt(a: &Ask, id: u64, p: &Palette, m: &Metrics) -> Div {
    let (mark, mark_color) = receipt_mark(a, p);
    let word = state_word(a);
    let said = a.reply.as_ref().and_then(|r| summary(&a.doc, r));
    let has_said = said.is_some();
    h_flex().w_full().bg(p.card_d).px(px(pad_x(m))).py(px(pad_y(m))).debug_selector(move || format!("ask-receipt {id}")).child(
        h_flex()
            .w_full()
            .h(px(head_h(m)))
            .gap(px(8.))
            .items_center()
            .font_family(fonts::SANS)
            .text_px(m.card_title)
            .whitespace_nowrap()
            .child(
                div().flex_none().size(px(Glyph::Needs.size())).flex().items_center().justify_center().child(glyph::draw(mark, mark_color)),
            )
            // The question reads first: the answer gives way to it, down
            // to half the line.
            .child(
                div()
                    .map(|d| question_box(d, has_said))
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_color(p.prose_t)
                    .debug_selector(move || format!("ask-question {id}"))
                    .child(shown(&a.doc.question, Lines::Single)),
            )
            .children(said.map(|s| {
                h_flex().min_w_0().flex_shrink(1.).gap(px(8.)).child(div().flex_none().text_color(p.dim_t).child("→")).child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(p.ink)
                        .debug_selector(move || format!("ask-said {id}"))
                        .child(s),
                )
            }))
            .child(div().flex_1().min_w(px(12.)))
            .child(
                div().flex_none().text_px(m.meta).text_color(p.dim_t).debug_selector(move || format!("ask-state {id} {word}")).child(word),
            ),
    )
}

impl AppView {
    /// A card's row ([`crate::turns::Kind::Ask`]).
    pub(crate) fn render_ask(&self, item: &Item, id: u64, p: &Palette, m: &Metrics) -> AnyElement {
        let Some(CardRow::Ask(a)) = item.card.as_deref() else { return div().into_any_element() };
        if whole(a) { card(a, id, p, m).into_any_element() } else { receipt(a, id, p, m).into_any_element() }
    }

    /// An answer's row ([`crate::turns::Kind::Answer`]): the user's turn,
    /// one line on the user's surface, with the you bar.
    pub(crate) fn render_answer(&self, item: &Item, id: u64, p: &Palette, m: &Metrics) -> AnyElement {
        let Some(CardRow::Answer(a)) = item.card.as_deref() else { return div().into_any_element() };
        let (verb, said) = match &a.reply {
            Reply::Skip => ("skipped", None),
            Reply::Change(_) => ("changed", summary(&a.doc, &a.reply)),
            Reply::Answer(_) => ("answered", summary(&a.doc, &a.reply)),
        };
        let has_said = said.is_some();
        h_flex()
            .relative()
            .w_full()
            .bg(p.user)
            .px(px(m.pad_x))
            .py(px(m.pad_y))
            .gap(px(8.))
            .items_center()
            .font_family(fonts::SANS)
            .text_px(m.body)
            .line_height(px(m.body_line))
            .whitespace_nowrap()
            .debug_selector(move || format!("ask-answer {id}"))
            .child(div().flex_none().child(glyph::draw(Glyph::Elbow, p.dim_u)))
            .child(div().flex_none().text_color(p.dim_u).child(verb))
            .child(
                div()
                    .map(|d| question_box(d, has_said))
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_color(p.dim_u)
                    .child(format!("«{}»", shown(&a.doc.question, Lines::Single))),
            )
            .children(said.map(|s| {
                h_flex()
                    .min_w_0()
                    .flex_shrink(1.)
                    .gap(px(8.))
                    .child(div().flex_none().text_color(p.dim_u).child("→"))
                    .child(div().min_w_0().overflow_hidden().text_ellipsis().text_color(p.ink).child(s))
            }))
            .child(crate::glyph::bar_at(p.you_mark, 1.))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use synaps_client::cards::{Answer, CardDoc, Choice, FieldValue, Reply};

    use super::{how, summary};

    fn doc(v: serde_json::Value) -> CardDoc {
        match cards_spec::parse_input(&v).unwrap() {
            cards_spec::Input::Create { doc, .. } => doc,
            cards_spec::Input::Close(_) => unreachable!(),
        }
    }

    /// A receipt says what was answered in a line: labels, the note, the
    /// fields by their labels.
    #[test]
    fn a_receipt_says_the_answer_in_a_line() {
        let d = doc(serde_json::json!({"question": "Ship?", "options": [{"label": "Yes", "note": "Why?"}, "No"],
            "fields": [{"id": "who", "label": "Reviewer"}, {"id": "ci", "label": "Run CI", "type": "check"}]}));
        let a = Answer {
            choice: Some(Choice::One("Yes".into())),
            note: Some("tests\npass".into()),
            fields: Some(vec![("who".into(), FieldValue::Text(String::new())), ("ci".into(), FieldValue::Check(true))]),
        };
        assert_eq!(summary(&d, &Reply::Answer(a)).as_deref(), Some("Yes · “tests pass” · Reviewer: — · Run CI: yes"));
        let none = Answer { choice: Some(Choice::Many(Vec::new())), ..Answer::default() };
        assert_eq!(summary(&d, &Reply::Change(none)).as_deref(), Some("(none)"));
        assert_eq!(summary(&d, &Reply::Skip), None);
        assert_eq!(how(&d), "Reply in your own words");
        let quick = doc(serde_json::json!({"question": "Ship?", "options": ["Yes", "No"]}));
        assert_eq!(how(&quick), "Reply with its number, or in your own words");
        let multi = doc(serde_json::json!({"question": "Which?", "options": ["A", "B"], "multi": true}));
        assert_eq!(how(&multi), "Reply with the numbers you pick, or in your own words");
    }
}
