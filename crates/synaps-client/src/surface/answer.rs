//! Answers: their values (§5.3), the `<ui_answer>` message (§3.6) and the
//! late-answer capsule, as a client reads them: a mirror of the parse half
//! of SynapsCLI's `agent_engine::ui::answer` (C1, `6e895d27`).
//!
//! - [`parse_answer`] validates values against the doc (§3.3 #11), as
//!   admission does: the fold reads a waited result's answer through it,
//!   and D3 can check a commit before it sends. Values are data for the
//!   model: they are refused, never rewritten (§4.3).
//! - [`Answer`] serializes with its keys in a fixed order and its fields in
//!   the doc's order.
//! - [`parse_answer_message`] reads the host's `<ui_answer>` message (it
//!   proves nothing: only a mark does, see [`super::fold`]). Only the host
//!   builds one, so the formatter is not mirrored.
//! - [`Capsule`]: a late answer's `<asked>` line, checked by
//!   [`Capsule::parse`].

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

pub use super::types::{Answer, AnswerStatus, Choice, FieldValue};

use super::doc::{CardDoc, FieldKind};
use super::sanitize::{Lines, char_len, value_control};
use super::*;

/// Validate a client's `values` for an `answer` (or `revise`) against the
/// doc (§3.3 #11, §5.3): types, required values, membership, lengths, no
/// C0/C1 controls (except `\n` and `\t` in `long`), at most 8 KiB.
///
/// Depth, then bytes (counted, never built), before anything else (Shady
/// #439 M2).
pub fn parse_answer(doc: &CardDoc, values: &Value) -> Result<Answer, UiError> {
    if deeper_than(values, ANSWER_MAX_DEPTH) {
        return Err(UiError::new("values", format!("nested deeper than {ANSWER_MAX_DEPTH} levels")));
    }
    if json_len_within(values, ANSWER_MAX_BYTES).is_none() {
        return Err(UiError::new("values", format!("over {ANSWER_MAX_BYTES} bytes")));
    }
    let Some(obj) = values.as_object() else {
        return Err(UiError::new("values", "must be an object"));
    };
    if let Some(k) = first_key(obj, |k, _| !matches!(k, "choice" | "note" | "fields")) {
        return Err(UiError::new(key_path(k), "unknown key; an answer has choice, note and fields"));
    }
    let choice = choice(doc, obj)?;
    let note = note(doc, obj, choice.as_ref())?;
    let fields = fields(doc, obj)?;
    Ok(Answer { choice, note, fields })
}

fn choice(doc: &CardDoc, obj: &Map<String, Value>) -> Result<Option<Choice>, UiError> {
    let Some(v) = obj.get("choice") else {
        return if doc.options.is_empty() { Ok(None) } else { Err(UiError::new("choice", "required")) };
    };
    if doc.options.is_empty() {
        return Err(UiError::new("choice", "this card has no options"));
    }
    let label = |v: &Value, path: &str| -> Result<String, UiError> {
        match v.as_str() {
            Some(l) if doc.option(l).is_some() => Ok(l.to_string()),
            Some(_) => Err(UiError::new(path, "not one of the option labels")),
            None => Err(UiError::new(path, "must be an option label")),
        }
    };
    if !doc.multi {
        return label(v, "choice").map(|l| Some(Choice::One(l)));
    }
    let Some(items) = v.as_array() else {
        return Err(UiError::new("choice", "must be a list of option labels"));
    };
    let mut out: Vec<String> = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let path = format!("choice[{i}]");
        let l = label(item, &path)?;
        if out.contains(&l) {
            return Err(UiError::new(path, "listed twice"));
        }
        out.push(l);
    }
    Ok(Some(Choice::Many(out)))
}

fn note(doc: &CardDoc, obj: &Map<String, Value>, choice: Option<&Choice>) -> Result<Option<String>, UiError> {
    let asks = match choice {
        Some(Choice::One(l)) => doc.option(l).is_some_and(|o| o.note.is_some()),
        _ => false,
    };
    let Some(v) = obj.get("note") else {
        return if asks { Err(UiError::new("note", "required for this option")) } else { Ok(None) };
    };
    if !asks {
        return Err(UiError::new("note", "the chosen option takes no note"));
    }
    let Some(n) = v.as_str() else {
        return Err(UiError::new("note", "must be a string"));
    };
    text_value(n, "note", Lines::Single, NOTE_MAX)?;
    if char_len(n) < NOTE_MIN {
        return Err(UiError::new("note", "must not be empty"));
    }
    Ok(Some(n.to_string()))
}

fn fields(doc: &CardDoc, obj: &Map<String, Value>) -> Result<Option<Vec<(String, FieldValue)>>, UiError> {
    let Some(v) = obj.get("fields") else {
        return if doc.fields.is_empty() { Ok(None) } else { Err(UiError::new("fields", "required: every field, by id")) };
    };
    if doc.fields.is_empty() {
        return Err(UiError::new("fields", "this card has no fields"));
    }
    let Some(given) = v.as_object() else {
        return Err(UiError::new("fields", "must be an object of values by field id"));
    };
    if let Some(k) = first_key(given, |k, _| !doc.fields.iter().any(|f| f.id == k)) {
        return Err(UiError::new(format!("fields.{}", key_path(k)), "no such field"));
    }
    let mut out = Vec::with_capacity(doc.fields.len());
    for f in &doc.fields {
        let path = format!("fields.{}", f.id);
        let Some(v) = given.get(&f.id) else {
            return Err(UiError::new(path, "missing; every field is sent (\"\" for an empty optional one)"));
        };
        let value = match (f.kind, v) {
            (FieldKind::Check, Value::Bool(b)) => FieldValue::Check(*b),
            (FieldKind::Check, _) => return Err(UiError::new(path, "a check takes true or false")),
            (kind, Value::String(s)) => {
                text_value(s, &path, kind.lines(), kind.max_chars().unwrap_or(TEXT_MAX))?;
                if s.is_empty() && !f.optional {
                    return Err(UiError::new(path, "required"));
                }
                FieldValue::Text(s.clone())
            }
            (kind, _) => return Err(UiError::new(path, format!("a {} field takes a string", kind.as_str()))),
        };
        out.push((f.id.clone(), value));
    }
    Ok(Some(out))
}

fn text_value(s: &str, path: &str, lines: Lines, max: usize) -> Result<(), UiError> {
    if let Some(c) = value_control(s, lines) {
        return Err(UiError::new(path, format!("holds U+{:04X} (a control character)", c as u32)));
    }
    let n = char_len(s);
    if n > max {
        return Err(UiError::new(path, format!("at most {max} characters (got {n})")));
    }
    Ok(())
}

// ── The answer message (§3.6) ────────────────────────────────────────────

/// One `<ui_answer>` message, as the host builds it and a client reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnswerMessage {
    pub card: String,
    pub answer_id: String,
    /// The sanitized question.
    pub q: String,
    pub status: AnswerStatus,
    /// The record's answer revision: 1, or 2–4 for a revision.
    pub rev: u32,
    pub values: Answer,
    /// A late answer's capsule; `None` on time.
    pub asked: Option<Capsule>,
}

/// Read a message the host built (`format_answer_message`:
/// `<ui_answer card="…" answer="…" q="…"[ status="skipped"| status="revised" rev="N"][ late="true"]>{json}[\n<asked …>…</asked>]</ui_answer>`).
/// `None` for anything else. Matching text is not evidence: only a
/// `_synaps_ui` mark is.
pub fn parse_answer_message(text: &str) -> Option<AnswerMessage> {
    let rest = text.strip_prefix("<ui_answer ")?.strip_suffix("</ui_answer>")?;
    let (attrs, body) = rest.split_once('>')?;
    let attrs = parse_attrs(attrs)?;
    let names: Vec<&str> = attrs.iter().map(|(k, _)| *k).collect();
    let get = |k: &str| attrs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string());
    let late = get("late").is_some();
    let tail: &[&str] = if late { &["late"] } else { &[] };
    let (status, rev) = match get("status").as_deref() {
        None => (AnswerStatus::Answered, 1),
        Some("skipped") => (AnswerStatus::Skipped, 1),
        Some("revised") => (AnswerStatus::Revised, get("rev")?.parse().ok().filter(|r| *r >= 2)?),
        Some(_) => return None,
    };
    let mut expected = vec!["card", "answer", "q"];
    match status {
        AnswerStatus::Answered => {}
        AnswerStatus::Skipped => expected.push("status"),
        AnswerStatus::Revised => expected.extend(["status", "rev"]),
    }
    expected.extend(tail);
    if names != expected || (late && get("late")? != "true") {
        return None;
    }
    let mut stream = serde_json::Deserializer::from_str(body).into_iter::<Answer>();
    let values = stream.next()?.ok()?;
    let after = &body[stream.byte_offset()..];
    let asked = match (late, after) {
        (false, "") => None,
        (true, a) => Some(Capsule::parse(a.strip_prefix('\n')?)?),
        (false, _) => return None,
    };
    Some(AnswerMessage { card: get("card")?, answer_id: get("answer")?, q: get("q")?, status, rev, values, asked })
}

/// `name="value"` pairs separated by single spaces; values hold no `"`.
fn parse_attrs(s: &str) -> Option<Vec<(&str, &str)>> {
    let mut out = Vec::new();
    let mut rest = s;
    loop {
        let (name, after) = rest.split_once("=\"")?;
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_lowercase()) {
            return None;
        }
        let (value, after) = after.split_once('"')?;
        out.push((name, value));
        if after.is_empty() {
            return Some(out);
        }
        rest = after.strip_prefix(' ')?;
    }
}

// ── The late-answer capsule (§3.6, R3-M1) ────────────────────────────────

/// A late answer's `<asked>` line (§3.6):
/// `<asked during="…"[ omitted="…"]>text</asked>`, at most 1 KiB, with no
/// `<` or `>` inside the tag and no `<` in the text. Only the host's
/// `build_capsule` makes one; [`Capsule::parse`] (also its `Deserialize`)
/// checks one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Capsule(String);

impl Capsule {
    pub fn parse(s: &str) -> Option<Self> {
        if s.len() > CAPSULE_MAX_BYTES {
            return None;
        }
        let rest = s.strip_prefix("<asked ")?.strip_suffix("</asked>")?;
        let (attrs, text) = rest.split_once('>')?;
        let names: Vec<&str> = parse_attrs(attrs)?.iter().map(|(k, _)| *k).collect();
        let ok = names == ["during"] || names == ["during", "omitted"];
        (ok && !attrs.contains('<') && !text.contains('<')).then(|| Capsule(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::ops::Deref for Capsule {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl PartialEq<&str> for Capsule {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

impl<'de> Deserialize<'de> for Capsule {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Capsule::parse(&s).ok_or_else(|| serde::de::Error::custom("not an <asked> capsule"))
    }
}

#[cfg(test)]
mod tests {
    use super::super::doc::{UiInput, parse_input};
    use super::*;
    use serde_json::json;

    fn doc(v: Value) -> CardDoc {
        match parse_input(&v).unwrap() {
            UiInput::Create { doc, .. } => doc,
            UiInput::Close(_) => unreachable!(),
        }
    }

    fn quick() -> CardDoc {
        doc(
            json!({"question": "Plan ready. Go ahead?", "options": ["Approve", {"label": "Revise", "note": "What should change?", "hint": "send notes"}]}),
        )
    }

    fn form() -> CardDoc {
        doc(json!({"question": "Release details", "fields": [
            {"id": "version", "label": "Version", "value": "0.13.0"},
            {"id": "notes", "label": "Notes", "type": "long", "optional": true},
            {"id": "publish", "label": "Publish to crates.io", "type": "check", "value": true}]}))
    }

    fn err(d: &CardDoc, v: Value) -> String {
        parse_answer(d, &v).unwrap_err().to_string()
    }

    #[test]
    fn shapes_in_answer_order() {
        let a = parse_answer(&form(), &json!({"fields": {"publish": true, "notes": "", "version": "0.13.0"}})).unwrap();
        assert_eq!(a.to_json(), r#"{"fields":{"version":"0.13.0","notes":"","publish":true}}"#);
        let a = parse_answer(&quick(), &json!({"note": "keep it", "choice": "Revise"})).unwrap();
        assert_eq!(a.to_json(), r#"{"choice":"Revise","note":"keep it"}"#);
        let multi = doc(json!({"question": "q", "options": ["a", "b", "c"], "multi": true}));
        assert_eq!(parse_answer(&multi, &json!({"choice": []})).unwrap().to_json(), r#"{"choice":[]}"#);
        assert_eq!(parse_answer(&multi, &json!({"choice": ["c", "a"]})).unwrap().to_json(), r#"{"choice":["c","a"]}"#);
        assert_eq!(Answer::skip().to_json(), "{}");
        // A stored answer reads back in its own order.
        let back: Answer = serde_json::from_str(r#"{"fields":{"version":"1","notes":"","publish":false}}"#).unwrap();
        assert_eq!(back.to_json(), r#"{"fields":{"version":"1","notes":"","publish":false}}"#);
        assert!(serde_json::from_str::<Answer>(r#"{"choice":"a","extra":1}"#).is_err());
        assert!(serde_json::from_str::<Answer>(r#"{"choice":"a","choice":"b"}"#).is_err());
    }

    #[test]
    fn refusals() {
        let q = quick();
        assert_eq!(err(&q, json!({})), "choice: required");
        assert_eq!(err(&q, json!({"choice": "approve"})), "choice: not one of the option labels");
        assert_eq!(err(&q, json!({"choice": ["Approve"]})), "choice: must be an option label");
        assert_eq!(err(&q, json!({"choice": "Revise"})), "note: required for this option");
        assert_eq!(err(&q, json!({"choice": "Approve", "note": "x"})), "note: the chosen option takes no note");
        assert_eq!(err(&q, json!({"choice": "Revise", "note": ""})), "note: must not be empty");
        assert_eq!(err(&q, json!({"choice": "Revise", "note": "a\nb"})), "note: holds U+000A (a control character)");
        assert_eq!(err(&q, json!({"choice": "Approve", "fields": {}})), "fields: this card has no fields");
        assert_eq!(err(&q, json!({"choice": "Approve", "blocks": 1})), "blocks: unknown key; an answer has choice, note and fields");
        assert_eq!(err(&q, json!("Approve")), "values: must be an object");
        let f = form();
        assert_eq!(
            err(&f, json!({"fields": {"version": "1", "notes": ""}})),
            "fields.publish: missing; every field is sent (\"\" for an empty optional one)"
        );
        assert_eq!(err(&f, json!({"fields": {"version": "", "notes": "", "publish": true}})), "fields.version: required");
        assert_eq!(
            err(&f, json!({"fields": {"version": "1", "notes": "", "publish": "yes"}})),
            "fields.publish: a check takes true or false"
        );
        assert_eq!(err(&f, json!({"fields": {"version": "1", "notes": "", "publish": true, "x": 1}})), "fields.x: no such field");
        assert_eq!(
            err(&f, json!({"fields": {"version": "a\tb", "notes": "", "publish": true}})),
            "fields.version: holds U+0009 (a control character)"
        );
        assert_eq!(
            err(&f, json!({"fields": {"version": "1", "notes": "a\u{1B}[31m", "publish": true}})),
            "fields.notes: holds U+001B (a control character)"
        );
        assert_eq!(err(&f, json!({"choice": "x", "fields": {}})), "choice: this card has no options");
        // `long` keeps `\n` and `\t`; user-typed bidi text is data, allowed.
        parse_answer(&f, &json!({"fields": {"version": "\u{202E}1", "notes": "a\n\tb", "publish": false}})).unwrap();
        let multi = doc(json!({"question": "q", "options": ["a", "b"], "multi": true}));
        assert_eq!(err(&multi, json!({"choice": ["a", "a"]})), "choice[1]: listed twice");
        assert_eq!(err(&multi, json!({"choice": "a"})), "choice: must be a list of option labels");
    }

    #[test]
    fn value_bounds_at_n_and_n_plus_1() {
        let f = form();
        let with = |field: &str, s: String| {
            let mut v = json!({"fields": {"version": "1", "notes": "", "publish": true}});
            v["fields"][field] = json!(s);
            v
        };
        parse_answer(&f, &with("version", "x".repeat(TEXT_MAX))).unwrap();
        assert_eq!(err(&f, with("version", "x".repeat(TEXT_MAX + 1))), "fields.version: at most 200 characters (got 201)");
        parse_answer(&f, &with("notes", "\u{1F600}".repeat(LONG_MAX))).unwrap();
        assert_eq!(err(&f, with("notes", "x".repeat(LONG_MAX + 1))), "fields.notes: at most 2000 characters (got 2001)");
        let q = quick();
        parse_answer(&q, &json!({"choice": "Revise", "note": "x".repeat(NOTE_MAX)})).unwrap();
        assert_eq!(err(&q, json!({"choice": "Revise", "note": "x".repeat(NOTE_MAX + 1)})), "note: at most 500 characters (got 501)");
        parse_answer(&q, &json!({"choice": "Revise", "note": "x".repeat(NOTE_MIN)})).unwrap();
    }

    /// 8 KiB of values: 8192 accepted, 8193 refused.
    #[test]
    fn values_bytes_at_n_and_n_plus_1() {
        let d = doc(
            json!({"question": "q", "fields": (0..5).map(|i| json!({"id": format!("f{i}"), "label": "L", "type": "long"})).collect::<Vec<_>>()}),
        );
        let at = |bytes: usize| {
            let mut v = json!({"fields": {"f0": "", "f1": "", "f2": "", "f3": "", "f4": ""}});
            let mut left = bytes - serde_json::to_vec(&v).unwrap().len();
            for i in 0..5 {
                let take = left.min(LONG_MAX);
                v["fields"][format!("f{i}")] = json!("x".repeat(take));
                left -= take;
            }
            assert_eq!(serde_json::to_vec(&v).unwrap().len(), bytes);
            v
        };
        parse_answer(&d, &at(ANSWER_MAX_BYTES)).unwrap();
        assert_eq!(err(&d, at(ANSWER_MAX_BYTES + 1)), "values: over 8192 bytes");
    }

    /// Depth 3 / 4, and a value of any depth is refused before it is
    /// serialized (Shady #439 M2).
    #[test]
    fn values_depth_at_n_and_n_plus_1() {
        let multi = doc(json!({"question": "q", "options": ["a", "b"], "multi": true}));
        parse_answer(&multi, &json!({"choice": ["a"]})).unwrap();
        assert_eq!(err(&multi, json!({"choice": [["a"]]})), "values: nested deeper than 3 levels");
        let mut deep = json!("a");
        for _ in 0..200_000 {
            deep = Value::Array(vec![deep]);
        }
        // Never cloned, dropped whole or put through `json!`: all recurse.
        let mut values = Map::new();
        values.insert("choice".into(), deep);
        let values = Value::Object(values);
        assert_eq!(parse_answer(&multi, &values).unwrap_err().to_string(), "values: nested deeper than 3 levels");
        let mut v = values;
        while let Some(inner) = match v {
            Value::Object(mut o) => o.remove("choice"),
            Value::Array(mut a) => a.pop(),
            _ => None,
        } {
            v = inner;
        }
    }

    /// Shady #439 M1: a client's hostile key never reaches the error raw.
    #[test]
    fn hostile_answer_keys() {
        let f = form();
        let k = "\u{1b}]0;x\u{7}\n\u{2066}";
        let mut top = json!({"fields": {"version": "1", "notes": "", "publish": true}});
        top[k] = json!(1);
        assert_eq!(err(&f, top), r#""\u001b]0;x\u0007\u000a\u2066": unknown key; an answer has choice, note and fields"#);
        let mut field = json!({"fields": {"version": "1", "notes": "", "publish": true}});
        field["fields"][k] = json!(1);
        let e = err(&f, field);
        assert_eq!(e, r#"fields."\u001b]0;x\u0007\u000a\u2066": no such field"#);
        assert!(crate::surface::tests::safe_line(&e));
    }

    fn message(values: Answer, status: AnswerStatus, rev: u32, asked: Option<Capsule>) -> AnswerMessage {
        AnswerMessage {
            card: "s_3f2a91c0d4e2a871".into(),
            answer_id: "a_6c1d4b7e9f203a58".into(),
            q: "Which approach?".into(),
            status,
            rev,
            values,
            asked,
        }
    }

    /// The engine's `message_forms`, read the other way: each form the host
    /// writes reads back as the message it was built from.
    #[test]
    fn message_forms_read_back() {
        let one = Answer { choice: Some(Choice::One("Refactor the parser".into())), ..Answer::default() };
        for (text, m) in [
            (
                r#"<ui_answer card="s_3f2a91c0d4e2a871" answer="a_6c1d4b7e9f203a58" q="Which approach?">{"choice":"Refactor the parser"}</ui_answer>"#,
                message(one.clone(), AnswerStatus::Answered, 1, None),
            ),
            (
                r#"<ui_answer card="s_3f2a91c0d4e2a871" answer="a_6c1d4b7e9f203a58" q="Which approach?" status="revised" rev="2">{"choice":"Refactor the parser"}</ui_answer>"#,
                message(one, AnswerStatus::Revised, 2, None),
            ),
            (
                r#"<ui_answer card="s_3f2a91c0d4e2a871" answer="a_6c1d4b7e9f203a58" q="Which approach?" status="skipped">{}</ui_answer>"#,
                message(Answer::skip(), AnswerStatus::Skipped, 1, None),
            ),
            (
                "<ui_answer card=\"s_3f2a91c0d4e2a871\" answer=\"a_6c1d4b7e9f203a58\" q=\"Which approach?\" late=\"true\">{}\n<asked during=\"t\"></asked></ui_answer>",
                message(Answer::skip(), AnswerStatus::Answered, 1, Capsule::parse("<asked during=\"t\"></asked>")),
            ),
        ] {
            assert_eq!(parse_answer_message(text), Some(m), "{text}");
        }
    }

    #[test]
    fn foreign_texts_do_not_parse() {
        for t in [
            "",
            "<ui_answer card=\"s_1\">{}</ui_answer>",
            "<ui_answer card=\"s_3f2a91c0d4e2a871\" answer=\"a_6c1d4b7e9f203a58\" q=\"q\">{} </ui_answer>",
            "<ui_answer card=\"s_3f2a91c0d4e2a871\" answer=\"a_6c1d4b7e9f203a58\" q=\"q\" status=\"maybe\">{}</ui_answer>",
            "<ui_answer card=\"s_3f2a91c0d4e2a871\" answer=\"a_6c1d4b7e9f203a58\" q=\"q\" status=\"revised\">{}</ui_answer>",
            "<ui_answer card=\"s_3f2a91c0d4e2a871\" answer=\"a_6c1d4b7e9f203a58\" q=\"q\" status=\"revised\" rev=\"1\">{}</ui_answer>",
            "<ui_answer answer=\"a_6c1d4b7e9f203a58\" card=\"s_3f2a91c0d4e2a871\" q=\"q\">{}</ui_answer>",
            "<ui_answer card=\"s_3f2a91c0d4e2a871\" answer=\"a_6c1d4b7e9f203a58\" q=\"q\">{\"x\":1}</ui_answer>",
            "<ui_answer card=\"s_3f2a91c0d4e2a871\" answer=\"a_6c1d4b7e9f203a58\" q=\"q\">{}\n<asked during=\"x\">y</asked></ui_answer>",
            "<ui_answer card=\"s_3f2a91c0d4e2a871\" answer=\"a_6c1d4b7e9f203a58\" q=\"q\" late=\"true\">{}</ui_answer>",
            "say <ui_answer card=\"s_3f2a91c0d4e2a871\" answer=\"a_6c1d4b7e9f203a58\" q=\"q\">{}</ui_answer>",
        ] {
            assert_eq!(parse_answer_message(t), None, "{t}");
        }
    }

    #[test]
    fn capsules_are_checked() {
        for ok in ["<asked during=\"t\"></asked>", "<asked during=\"\" omitted=\"hints\">x \\u003c y</asked>"] {
            assert!(Capsule::parse(ok).is_some(), "{ok}");
            let c: Capsule = serde_json::from_value(json!(ok)).unwrap();
            assert_eq!(serde_json::to_value(&c).unwrap(), json!(ok));
        }
        let long = format!("<asked during=\"t\">{}</asked>", "x".repeat(CAPSULE_MAX_BYTES));
        for bad in [
            "",
            "<asked during=\"t\">a<b</asked>",
            "<asked during=\"<t\">a</asked>",
            "<asked during=\"t\" late=\"true\">a</asked>",
            "<asked omitted=\"hints\" during=\"t\">a</asked>",
            "<asked during=\"t\">a</asked> ",
            long.as_str(),
        ] {
            assert_eq!(Capsule::parse(bad), None, "{bad}");
            assert!(serde_json::from_value::<Capsule>(json!(bad)).is_err());
        }
    }
}
