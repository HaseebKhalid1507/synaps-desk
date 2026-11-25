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
use cards_spec::memo::State as MemoState;
use gpui_kit::component::input::{Input, Textarea};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use synaps_client::cards::{Answer, Ask, CardDoc, CardRow, Choice, DefaultChoice, FieldKind, FieldValue, Mode, Reply, State};
use synaps_client::transcript::{Item, ItemKind};

use crate::app::{AppView, display};
use crate::deliver::Painted;
use crate::fonts::{self, TextPx as _};
use crate::glyph::{self, Glyph};
use crate::pane::PaneId;
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

/// What an open card's controls may do now ([`AppView::render_ask`]):
/// `None` on a card drawn read-only (asking, in a worker's view, no
/// session attached).
pub(crate) struct Act {
    /// The card as painted: what a click names ([`crate::deliver`]).
    pub(crate) painted: Painted,
    /// The pane it is drawn in: its listeners act there.
    pub(crate) pane: PaneId,
    /// Its controls act: the window may send, and no compaction runs. The
    /// arm and the binding are checked on the click.
    pub(crate) can: bool,
    /// The footer's line, instead of how to answer: why the controls don't
    /// act, or what the last click met.
    pub(crate) line: Option<String>,
}

/// How to answer an open card by mouse, its footer's line.
pub(crate) fn how_click(doc: &CardDoc) -> &'static str {
    match (doc.mode(), doc.multi, doc.options.is_empty()) {
        (Mode::Quick, _, _) => "Click one, or reply in your own words",
        (Mode::Form, true, false) => "Pick any, then Send, or reply in your own words",
        (Mode::Form, _, _) => "Fill it in and Send, or reply in your own words",
    }
}

/// When each control of each card was first painted in view, actionable,
/// in a pane: by row, and the control's key ([`control`]).
pub(crate) type Shown = std::rc::Rc<std::cell::RefCell<std::collections::HashMap<(u64, String), std::time::Instant>>>;

/// The key of control `what` (`option <label>`, `field <id>`, `send`,
/// `skip`, `retry`, `resend`) of card `card`: a card bound again is a new
/// card, whose controls arm again.
pub(crate) fn control(card: &cards_spec::CardId, what: &str) -> String {
    format!("{card} {what}")
}

/// A probe on control `key` of row `row` (a child of the control, which
/// must be positioned): the first frame that paints any of it inside its
/// clip, the list's view (and a growing row's mask), stamps `shown`; the
/// control acts [`crate::app::ARM`] after (Shady #81 P2-3). Not when the
/// row is built: GPUI builds rows above and under the view to measure
/// them (its overdraw) and paints none of them; nor a frame whose clip
/// leaves none of the control in view.
pub(crate) fn probe(shown: &Shown, row: u64, key: String) -> impl IntoElement {
    let shown = shown.clone();
    canvas(
        |_, _, _| {},
        move |bounds, (), window, _| {
            let seen = bounds.intersect(&window.content_mask().bounds);
            if seen.size.width > px(0.) && seen.size.height > px(0.) {
                shown.borrow_mut().entry((row, key)).or_insert_with(crate::app::arm_now);
            }
        },
    )
    .absolute()
    .inset_0()
}

/// The card's buttons, square and outline-free: a ghost on the card (Skip,
/// Retry) or the accent's fill (Send, Resend). Hovered a step on, pressed
/// down [`crate::motion::PRESS`]; disabled, flat and dim.
fn button(name: &'static str, row: u64, text: &'static str, fill: bool, enabled: bool, p: &Palette, m: &Metrics) -> Stateful<Div> {
    let (bg, hi, press, fg) = if fill { (p.btn_fill, p.btn_hi, p.btn_press, p.acc_ink) } else { (p.card_d, p.hover, p.hover, p.ink) };
    let (bg, fg) = if enabled { (bg, fg) } else { (if fill { p.well_d } else { p.card_d }, p.dim_k) };
    h_flex()
        .id((name, row))
        .relative()
        .flex_none()
        .h(px(crate::fit::button_h(m)))
        .px(px(14.))
        .items_center()
        .bg(bg)
        .when(enabled, |d| d.cursor_pointer().hover(move |d| d.bg(hi)).active(move |s| s.bg(press).top(px(crate::motion::PRESS))))
        .child(div().font_family(fonts::SANS).font_weight(FontWeight::SEMIBOLD).text_px(m.label).text_color(fg).child(text))
        .debug_selector(move || format!("{name} {row}{}", if enabled { "" } else { " off" }))
}

/// A text field's well, for a static value or an input in it.
fn well(m: &Metrics, p: &Palette, long: bool) -> Div {
    let r = m.r;
    div()
        .w_full()
        .min_h(px((28. * r).round()))
        .px(px((8. * r).round()))
        .py(px((4. * r).round()))
        .bg(p.well_d)
        .font_family(fonts::MONO)
        .text_px(m.mono)
        .line_height(px(m.mono_line))
        .when(long, |d| d.min_h(px(3. * m.mono_line + (8. * r).round())))
}

impl AppView {
    /// The card, open (or asking): head, question, detail, options, fields,
    /// footer. With `act`, its controls answer it: an option click (a quick
    /// card's answer, a form's pick), a check, a field to type in, Skip and
    /// Send ([`crate::answer`]).
    fn card(&self, a: &Ask, id: u64, p: &Palette, m: &Metrics, act: Option<&Act>, cx: &mut Context<Self>) -> Div {
        let doc = &a.doc;
        let r = m.r;
        let open = a.state == State::Open;
        let word = state_word(a);
        let draft = act.and_then(|_| self.card_session()).and_then(|s| self.card_drafts.get(&(s, a.tool_id.clone())));
        // What is picked: the draft's, or a form's default before any click.
        let picks: Vec<String> = match (act, draft) {
            (Some(_), Some(d)) => d.picked.clone(),
            (Some(_), None) => crate::answer::default_picks(doc),
            (None, _) => Vec::new(),
        };
        let can = act.is_some_and(|x| x.can);
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
                .children(crate::detail::lines(&d).into_iter().map(|l| detail_line(l, p, m)))
        });
        let key = (20. * r).round();
        let pad = (6. * r).round();
        let form = doc.mode() == Mode::Form;
        let options = (!doc.options.is_empty()).then(|| {
            v_flex().mt(px((10. * r).round())).gap(px((2. * r).round())).children(doc.options.iter().enumerate().map(|(i, o)| {
                let n = i + 1;
                let picked = picks.contains(&o.label);
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
                let (cap_bg, cap_fg) = if picked { (p.acc, p.acc_ink) } else { (p.well_d, p.dim_t) };
                let row = h_flex()
                    .id(("ask-option", id * 64 + n as u64))
                    .relative()
                    .min_h(px((30. * r).round()))
                    .mx(px(-pad))
                    .px(px(pad))
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
                            .bg(cap_bg)
                            .font_family(fonts::MONO)
                            .text_px(m.meta)
                            .text_color(cap_fg)
                            .when(picked, |d| d.debug_selector(move || format!("ask-picked {id} {n}")))
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
                            .text_color(if picked { p.ink } else { p.prose_t })
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
                    });
                let row = match act.filter(|_| can) {
                    Some(x) => {
                        let (pane, painted, label) = (x.pane, x.painted.clone(), o.label.clone());
                        row.child(probe(&self.panes.cur.ask_shown, id, control(&x.painted.id, &format!("option {label}"))))
                            .cursor_pointer()
                            .hover(|d| d.bg(p.hover))
                            // Pressed: the accent's soft fill, down a pixel.
                            .active(|s| s.bg(p.acc_soft).top(px(crate::motion::PRESS)))
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                this.in_pane(pane, |this| this.ask_pick(id, &painted, &label, window, cx));
                            }))
                    }
                    None => row,
                };
                // A picked option that asks for a note: its field, under it.
                let note_field = draft.filter(|_| picked && o.note.is_some() && !doc.multi).and_then(|d| d.note.as_ref()).map(|e| {
                    div().pl(px(key + (10. * r).round())).pb(px((4. * r).round())).debug_selector(move || format!("ask-note {id}")).child(
                        well(m, p, false)
                            .child(Input::new(e).appearance(false).text_px(m.mono).line_height(px(m.mono_line)).text_color(p.ink)),
                    )
                });
                v_flex().child(row).children(note_field)
            }))
        });
        let fields = (!doc.fields.is_empty()).then(|| {
            v_flex().mt(px((10. * r).round())).gap(px((8. * r).round())).children(doc.fields.iter().map(|f| {
                let fid = f.id.clone();
                let listen = act.filter(|_| can).map(|x| {
                    let (pane, painted, fid) = (x.pane, x.painted.clone(), f.id.clone());
                    let key = control(&painted.id, &format!("field {fid}"));
                    let l = cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.in_pane(pane, |this| this.ask_field(id, &painted, &fid, window, cx));
                    });
                    (l, key)
                });
                let value = draft.and_then(|d| d.value(doc, &f.id, cx)).unwrap_or_else(|| f.value.clone());
                let value: AnyElement = match (&f.kind, &value) {
                    (FieldKind::Check, v) => {
                        let on = *v == FieldValue::Check(true);
                        let row = h_flex()
                            .id(("ask-check", id * 64 + f.id.len() as u64))
                            .gap(px(8.))
                            .items_center()
                            .child(
                                div()
                                    .flex_none()
                                    .size(px(12.))
                                    .bg(p.well_d)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .when(on, |d| d.child(div().size(px(6.)).bg(p.acc))),
                            )
                            .child(div().text_px(m.card_title).text_color(p.prose_t).child(shown(&f.label, Lines::Single)));
                        match listen {
                            Some((l, key)) => row
                                .relative()
                                .child(probe(&self.panes.cur.ask_shown, id, key))
                                .cursor_pointer()
                                .on_click(l)
                                .into_any_element(),
                            None => row.into_any_element(),
                        }
                    }
                    (_, FieldValue::Text(t)) => {
                        let long = f.kind == FieldKind::Long;
                        let input: Option<AnyElement> = draft.and_then(|d| {
                            if long {
                                d.longs.get(&f.id).map(|e| {
                                    Textarea::new(e)
                                        .appearance(false)
                                        .text_px(m.mono)
                                        .line_height(px(m.mono_line))
                                        .text_color(p.ink)
                                        .into_any_element()
                                })
                            } else {
                                d.lines.get(&f.id).map(|e| {
                                    Input::new(e)
                                        .appearance(false)
                                        .text_px(m.mono)
                                        .line_height(px(m.mono_line))
                                        .text_color(p.ink)
                                        .into_any_element()
                                })
                            }
                        });
                        let field = match input {
                            Some(input) => well(m, p, false).child(input).into_any_element(),
                            None => {
                                let (text, color) = match (t.is_empty(), &f.placeholder) {
                                    (false, _) => (shown(t, f.kind.lines()), p.ink),
                                    (true, Some(ph)) => (shown(ph, Lines::Single), p.dim_t),
                                    (true, None) => (String::new(), p.dim_t),
                                };
                                let w = well(m, p, long)
                                    .id(("ask-text", id * 64 + f.id.len() as u64))
                                    .text_color(color)
                                    .children(text.split('\n').map(|l| div().min_h(px(m.mono_line)).child(l.to_string())));
                                match listen {
                                    Some((l, key)) => w
                                        .relative()
                                        .child(probe(&self.panes.cur.ask_shown, id, key))
                                        .cursor_text()
                                        .hover(|d| d.bg(p.hover))
                                        .on_click(l)
                                        .into_any_element(),
                                    None => w.into_any_element(),
                                }
                            }
                        };
                        // An optional field says so: it may be left empty.
                        let optional = f.optional.then(|| {
                            let fid = f.id.clone();
                            div().flex_none().debug_selector(move || format!("ask-optional {id} {fid}")).child("· optional")
                        });
                        v_flex()
                            .gap(px((4. * r).round()))
                            .child(
                                h_flex()
                                    .gap(px(6.))
                                    .text_px(m.meta)
                                    .text_color(p.dim_t)
                                    .child(div().min_w_0().child(shown(&f.label, Lines::Single)))
                                    .children(optional),
                            )
                            .child(field)
                            .into_any_element()
                    }
                    (_, FieldValue::Check(_)) => div().into_any_element(),
                };
                div().font_family(fonts::SANS).debug_selector(move || format!("ask-field {id} {fid}")).child(value)
            }))
        });
        // Send, where a click alone isn't the answer: a form, or a quick
        // card's option that asks for a note, picked.
        let sends = form || draft.is_some_and(|d| d.picked.iter().any(|l| doc.option(l).is_some_and(|o| o.note.is_some())));
        let reply = act.map(|_| match draft {
            Some(d) => d.reply(doc, cx),
            None => crate::answer::Draft::new(doc).reply(doc, cx),
        });
        let ready = reply.as_ref().is_some_and(Result::is_ok);
        let (text, warn) = match act {
            None => (how(doc).to_string(), false),
            Some(x) => match (&x.line, sends, &reply) {
                (Some(l), _, _) => (l.clone(), true),
                (None, true, Some(Err(needs))) if draft.is_some() => (needs.clone(), false),
                _ => (how_click(doc).to_string(), false),
            },
        };
        let foot = (a.state != State::Pending).then(|| {
            let buttons = act.map(|x| {
                let skip = {
                    let (pane, painted) = (x.pane, x.painted.clone());
                    let key = control(&painted.id, "skip");
                    button("ask-skip", id, "Skip", false, can, p, m).when(can, |b| {
                        b.child(probe(&self.panes.cur.ask_shown, id, key)).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.in_pane(pane, |this| this.ask_skip(id, &painted, cx));
                        }))
                    })
                };
                let send = sends.then(|| {
                    let (pane, painted) = (x.pane, x.painted.clone());
                    // Armed from its first frame on an actionable card, lit or
                    // not: a form made whole is the user's own doing.
                    let key = control(&painted.id, "send");
                    button("ask-send", id, "Send", true, can && ready, p, m)
                        .when(can, |b| b.child(probe(&self.panes.cur.ask_shown, id, key)))
                        .when(can && ready, |b| {
                            b.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.in_pane(pane, |this| this.ask_submit(id, &painted, cx));
                            }))
                        })
                });
                h_flex().flex_none().gap(px(8.)).child(skip).children(send)
            });
            h_flex()
                .mt(px((10. * r).round()))
                .gap(px(12.))
                .items_center()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .font_family(fonts::SANS)
                        .text_px(m.meta)
                        .line_height(line(m.meta))
                        .text_color(if warn { p.prose_t } else { p.dim_t })
                        .debug_selector(move || format!("ask-footer {id}"))
                        .child(text),
                )
                .children(buttons)
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
}

/// What an answer says, whole: each pick, the note, then each field's
/// `Label: value`, a value's own lines kept ([`shown`], §5.2). A receipt
/// opened shows these.
pub(crate) fn parts(doc: &CardDoc, reply: &Reply) -> Vec<String> {
    let a: &Answer = match reply {
        Reply::Answer(a) | Reply::Change(a) => a,
        Reply::Skip => return Vec::new(),
    };
    let mut out: Vec<String> = Vec::new();
    match &a.choice {
        Some(Choice::One(l)) => out.push(shown(l, Lines::Single)),
        Some(Choice::Many(ls)) if ls.is_empty() => out.push("(none)".into()),
        Some(Choice::Many(ls)) => out.push(shown(&ls.join(", "), Lines::Single)),
        None => {}
    }
    if let (Some(note), Some(Choice::One(l))) = (&a.note, &a.choice) {
        let prompt = doc.option(l).and_then(|o| o.note.as_deref()).unwrap_or("Note");
        out.push(format!("{}: {}", shown(prompt, Lines::Single), shown(note, Lines::Single)));
    }
    for (id, v) in a.fields.iter().flatten() {
        let f = doc.field(id);
        let label = f.map_or(id.as_str(), |f| f.label.as_str());
        let value = match v {
            FieldValue::Check(true) => "yes".to_string(),
            FieldValue::Check(false) => "no".to_string(),
            FieldValue::Text(t) if t.is_empty() => "—".to_string(),
            FieldValue::Text(t) => shown(t, f.map_or(Lines::Single, |f| f.kind.lines())),
        };
        out.push(format!("{}: {value}", shown(label, Lines::Single)));
    }
    out
}

/// A receipt that opens: its hover group (its chevron brightens).
const RECEIPT: &str = "ask-receipt";

/// The card's receipt: its head line, the question → the answer, the state
/// word at its end. `word` and `tone`: the state as the memo says it while
/// an answer of this desk's is on its way (else the fold's). `open`: the
/// whole answer under it, a line each ([`parts`]).
pub(crate) fn receipt(a: &Ask, reply: Option<&Reply>, look: Look, id: u64, p: &Palette, m: &Metrics) -> Div {
    let at = |d: Div| if look.opens { d.group(RECEIPT) } else { d };
    let (mark, mark_color) = match look.tone {
        Tone::Fold => receipt_mark(a, p),
        Tone::Out => (Glyph::Needs, p.acc),
        Tone::Lost => (Glyph::Needs, p.warn),
    };
    let word = look.word.unwrap_or_else(|| state_word(a));
    let said = reply.and_then(|r| summary(&a.doc, r));
    let has_said = said.is_some();
    let whole: Vec<String> = if look.open { reply.map(|r| parts(&a.doc, r)).unwrap_or_default() } else { Vec::new() };
    let word_color = match look.tone {
        Tone::Fold => p.dim_t,
        Tone::Out => p.acc,
        Tone::Lost => p.warn,
    };
    v_flex()
        .map(at)
        .w_full()
        .bg(p.card_d)
        .px(px(pad_x(m)))
        .py(px(pad_y(m)))
        .debug_selector(move || format!("ask-receipt {id}"))
        .child(
            h_flex()
                .w_full()
                .h(px(head_h(m)))
                .gap(px(8.))
                .items_center()
                .font_family(fonts::SANS)
                .text_px(m.card_title)
                .whitespace_nowrap()
                .child(
                    div()
                        .flex_none()
                        .size(px(Glyph::Needs.size()))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(glyph::draw(mark, mark_color)),
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
                .children(said.filter(|_| !look.open).map(|s| {
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
                    div()
                        .flex_none()
                        .text_px(m.meta)
                        .text_color(word_color)
                        .debug_selector(move || format!("ask-state {id} {word}"))
                        .child(word),
                )
                .when(look.opens, |d| {
                    let g = if look.open { Glyph::FoldOpen } else { Glyph::Fold };
                    let open = look.open;
                    d.child(
                        crate::glyph::reveal(g, RECEIPT, RECEIPT, true, p.dim_t, p.ink)
                            .debug_selector(move || format!("ask-opens {id} {}", if open { "open" } else { "shut" })),
                    )
                }),
        )
        .when(!whole.is_empty(), |d| {
            d.child(
                v_flex()
                    .pl(px(Glyph::Needs.size() + 8.))
                    .pt(px((4. * m.r).round()))
                    .gap(px((2. * m.r).round()))
                    .font_family(fonts::SANS)
                    .text_px(m.label)
                    .line_height(line(m.label))
                    .text_color(p.ink)
                    .debug_selector(move || format!("ask-whole {id}"))
                    .children(whole.into_iter().map(|l| {
                        div().w_full().whitespace_normal().children(l.split('\n').map(|s| div().min_h(line(m.label)).child(s.to_string())))
                    })),
            )
        })
}

/// A line of a card's detail, markdown-lite ([`crate::detail`]): bold in
/// semibold, italic, inline code as the prose draws it (`code_in` on its
/// `well` chip); a list item's mark hangs in its own column, indented by
/// its depth.
fn detail_line(l: crate::detail::Line, p: &Palette, m: &Metrics) -> Div {
    let looks = l.looks.into_iter().map(|(range, k)| {
        let style = HighlightStyle {
            font_weight: k.bold.then_some(FontWeight::SEMIBOLD),
            font_style: k.italic.then_some(FontStyle::Italic),
            color: k.code.then_some(p.code_in),
            background_color: k.code.then_some(p.well),
            ..Default::default()
        };
        (range, style)
    });
    let text = StyledText::new(l.text).with_highlights(looks);
    match l.lead {
        Some(lead) => h_flex()
            .items_start()
            .min_h(line(m.label))
            .pl(px((14. * m.r).round() * l.depth as f32))
            .child(div().flex_none().w(px((18. * m.r).round())).text_color(p.dim_t).child(lead))
            .child(div().flex_1().min_w_0().child(text)),
        None => div().min_h(line(m.label)).child(text),
    }
}

/// The line of a card this window can't answer, from the input gate's
/// reason (`Live::input_gate`): an exclusive session's other clients are
/// told whose it is, and how to take it (§3.2).
fn read_only(gate: &str, l: &crate::live::Live) -> String {
    if let Some(a) = l.active.as_ref()
        && a.input == synaps_client::ClientInput::Exclusive
        && a.mode != synaps_client::AttachMode::Observe
        && let Some(owner) = a.owner.filter(|o| *o != a.client)
    {
        return format!("Answer from client #{owner}, which owns this session's input · /takeover to answer here");
    }
    let mut c = gate.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// How a receipt reads: the fold's state, or an answer of this desk's on
/// its way (the memo's, [`crate::deliver::Memos::words`]).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Look {
    pub(crate) word: Option<&'static str>,
    pub(crate) tone: Tone,
    /// The whole answer shown under it.
    pub(crate) open: bool,
    /// A click opens or closes it: a chevron after its state says so,
    /// brighter under the pointer.
    pub(crate) opens: bool,
}

/// A receipt's tone: the fold's, on its way (the accent), or not
/// confirmed / not sent (the warning).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Tone {
    #[default]
    Fold,
    Out,
    Lost,
}

impl AppView {
    /// A card's row ([`crate::turns::Kind::Ask`]): the card while it is
    /// open; once this desk answered it, its receipt with where the answer
    /// is (§3.5's table) and, unconfirmed or not sent, the cause and
    /// Resend or Retry; else the fold's receipt, which a click opens.
    pub(crate) fn render_ask(&self, item: &Item, id: u64, p: &Palette, m: &Metrics, cx: &mut Context<Self>) -> AnyElement {
        let Some(CardRow::Ask(a)) = item.card.as_deref() else { return div().into_any_element() };
        let pane = self.panes.cur.id;
        let session = self.card_session();
        let l = &self.model.lives[self.panes.cur.live];
        let fold = session.as_ref().and_then(|_| l.transcript.cards().card(&a.tool_id)).filter(|c| c.id == a.id);
        let entry = session.as_deref().zip(fold).and_then(|(s, c)| Some((s, self.model.memos.entry(s, c)?)));
        let open = session.as_ref().is_some_and(|s| self.card_open.contains(&(s.clone(), a.tool_id.clone())));
        let said = session.as_ref().and_then(|s| self.card_drafts.get(&(s.clone(), a.tool_id.clone()))).and_then(|d| d.said.clone());
        // An answer of this desk's on its way, or not confirmed.
        if let Some((s, e)) = entry.filter(|(_, e)| e.state != MemoState::InChat || whole(a)) {
            let (word, cause) = self.model.memos.words(s, e);
            // Taken in, its turn over, or queued or held, and no settled
            // view since dropped events: not confirmed, and nothing to
            // offer but "Check again" until one settles.
            let unchecked = self.model.memos.unchecked(s, e);
            let lost = matches!(e.state, MemoState::Unconfirmed | MemoState::NotSent) || unchecked;
            let look = Look {
                word: Some(word),
                tone: if lost {
                    Tone::Lost
                } else if e.state == MemoState::InChat {
                    Tone::Fold
                } else {
                    Tone::Out
                },
                open,
                opens: false,
            };
            let mut out = receipt(a, Some(&e.reply), look, id, p, m);
            if lost && let Some(painted) = Painted::of(a) {
                // Dropped events: nothing is checked until the history is
                // in again and an event has said the slot (`deliver`'s
                // "Dropped events"); past the wait, the agent may have it,
                // and "Check again" where Resend would be.
                let gate = l
                    .input_gate()
                    .err()
                    .map(str::to_string)
                    .or_else(|| crate::deliver::recovering(&l.transcript).then(|| self.model.memos.answer_gap(s).to_string()));
                let again = gate.as_deref() == Some(crate::deliver::MAY_HAVE_IT);
                let resend = e.state == MemoState::Unconfirmed;
                let can = gate.is_none()
                    && fold.is_some_and(|c| {
                        if resend {
                            session.as_deref().is_some_and(|s| self.model.memos.may_resend(s, &painted.id, l.transcript.cards()))
                        } else {
                            c.answerable()
                        }
                    });
                let (name, label) = if resend { ("ask-resend", "Resend") } else { ("ask-retry", "Retry") };
                let key = control(&painted.id, if resend { "resend" } else { "retry" });
                let btn = if again {
                    let key = control(&painted.id, "check");
                    Some(
                        button("ask-check", id, "Check again", false, true, p, m)
                            .child(probe(&self.panes.cur.ask_shown, id, key))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.in_pane(pane, |this| this.ask_check_again(id, &painted, cx));
                            })),
                    )
                } else {
                    (!unchecked).then(|| {
                        button(name, id, label, resend, can, p, m).when(can, |b| {
                            b.child(probe(&self.panes.cur.ask_shown, id, key)).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.in_pane(pane, |this| {
                                    if resend { this.ask_resend(id, &painted, cx) } else { this.ask_retry(id, &painted, cx) }
                                });
                            }))
                        })
                    })
                };
                let why = said.or(gate).or(cause).unwrap_or_default();
                out = out.child(
                    h_flex()
                        .mt(px((8. * m.r).round()))
                        .pl(px(Glyph::Needs.size() + 8.))
                        .gap(px(12.))
                        .items_center()
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .font_family(fonts::SANS)
                                .text_px(m.meta)
                                .line_height(line(m.meta))
                                .child(div().text_color(p.prose_t).debug_selector(move || format!("ask-cause {id}")).child(why))
                                .when(resend, |d| d.child(div().text_color(p.dim_t).child("If it did arrive, the agent reads it twice."))),
                        )
                        .children(btn),
                );
            }
            return out.into_any_element();
        }
        if whole(a) {
            let act = Painted::of(a).filter(|_| a.state == State::Open).zip(session.as_ref()).map(|(painted, _)| {
                let why = match l.input_gate() {
                    Err(e) => Some(read_only(e, l)),
                    Ok(()) if l.transcript.compacting() => Some(crate::deliver::COMPACTING.to_string()),
                    // Dropped events: the history in again, and the slot
                    // said, first.
                    Ok(()) if crate::deliver::recovering(&l.transcript) => {
                        Some(session.as_deref().map_or(crate::deliver::RESYNCING, |s| self.model.memos.gap(s)).to_string())
                    }
                    // The live turn's alone: its controls wait for the
                    // history to confirm its binding (Shady #81 P2-1).
                    Ok(()) if !painted.confirmed(&l.transcript) => Some(crate::deliver::CONFIRMING.to_string()),
                    Ok(()) => None,
                };
                Act { painted, pane, can: why.is_none(), line: why.or(said) }
            });
            return self.card(a, id, p, m, act.as_ref(), cx).into_any_element();
        }
        let tool = a.tool_id.clone();
        let reply = a.reply.as_ref();
        let opens = session.is_some() && reply.is_some_and(|r| !parts(&a.doc, r).is_empty());
        let r = receipt(a, reply, Look { open, opens, ..Look::default() }, id, p, m);
        if opens {
            r.id(("ask-receipt", id))
                .cursor_pointer()
                .hover(|d| d.bg(p.hover))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.in_pane(pane, |this| this.ask_open(id, &tool, cx)).unwrap_or_default()
                }))
                .into_any_element()
        } else {
            r.into_any_element()
        }
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
