//! C1's fixture corpus against this mirror (agent-ui §4.3: parity is a
//! fixture contract). `tests/fixtures/ui/` is SynapsCLI's
//! `crates/agent-engine/tests/fixtures/ui/` at `6e895d27`, unchanged; each
//! test below is its namesake in the engine's `tests/ui_card_model.rs`,
//! less what only the host does (the contract digest, building the
//! message and its capsule), plus the fold of each example as history.

use std::path::PathBuf;

use serde_json::{Value, json};

use super::answer::{AnswerMessage, Capsule, parse_answer, parse_answer_message};
use super::doc::{CardDoc, UiInput, parse_input, revalidate};
use super::fold::{Outcome, fold_answer, fold_history};
use super::sanitize::{Lines, fold_key, refused_char, ui_sanitize};
use super::*;

fn corpus() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/ui"))
}

fn read_dir(kind: &str) -> Vec<(String, Value)> {
    let mut out: Vec<(String, Value)> = std::fs::read_dir(corpus().join(kind))
        .unwrap_or_else(|e| panic!("{kind}: {e}"))
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .map(|p| {
            let name = p.file_stem().unwrap().to_string_lossy().into_owned();
            let v: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap_or_else(|e| panic!("{name}: {e}"));
            (name, v)
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn read(name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(corpus().join(name)).unwrap()).unwrap()
}

fn status(s: &str) -> AnswerStatus {
    serde_json::from_value(json!(s)).unwrap()
}

fn bytes(v: &Value) -> usize {
    serde_json::to_vec(v).unwrap().len()
}

/// A refusal is one line, at most 322 characters, with nothing that can
/// hide or reorder text (Shady #439 M1).
fn one_safe_line(e: &str) -> bool {
    e.chars().count() <= ERROR_PATH_MAX + 2 + ERROR_MESSAGE_MAX && ui_sanitize(e, Lines::Single) == e && refused_char(e).is_none()
}

/// The corpus is the whole of C1's: every Appendix A example, every
/// directory, at least the engine's counts.
#[test]
fn every_appendix_a_example_is_in_the_corpus() {
    let valid: Vec<String> = read_dir("valid").into_iter().map(|(n, _)| n).collect();
    for a in ["a1_", "a2_", "a3_", "a4_", "a5_"] {
        assert!(valid.iter().any(|n| n.starts_with(a)), "{a}");
    }
    let invalid: Vec<String> = read_dir("invalid").into_iter().map(|(n, _)| n).collect();
    for a in [
        "a_ten_options",
        "a_duplicate_label",
        "a_default_not_an_option",
        "a_note_with_multi",
        "a_field_id_uppercase",
        "a_check_value_string",
        "a_close_with_question",
        "a_unknown_key_blocks",
        "a_question_121_chars",
        "a_question_bidi_override",
    ] {
        assert!(invalid.iter().any(|n| n == a), "{a}");
    }
    assert_eq!((valid.len(), invalid.len(), read_dir("raw").len()), (16, 33, 12), "the corpus at 6e895d27");
}

fn assistant_call(id: &str, input: &Value) -> Value {
    json!({"role": "assistant", "content": [{"type": "tool_use", "id": id, "name": "ui", "input": input}]})
}

fn tool_result(id: &str, result: &Value, mark: Option<Value>) -> Value {
    let mut m = json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": id, "content": result.to_string()}]});
    if let Some(mark) = mark {
        m[MARK_KEY] = mark;
    }
    m
}

fn user_text(text: &str, mark: Option<Value>) -> Value {
    let mut m = json!({"role": "user", "content": text});
    if let Some(mark) = mark {
        m[MARK_KEY] = mark;
    }
    m
}

#[test]
fn valid_fixtures() {
    let all = read_dir("valid");
    assert!(all.len() >= 9);
    for (name, f) in all {
        let parsed = parse_input(&f["input"]).unwrap_or_else(|e| panic!("{name}: {e}"));
        if let Some(close) = f["close"].as_str() {
            assert_eq!(parsed, UiInput::Close(close.into()), "{name}");
            assert_eq!(f["result"], json!({"status": "closed", "id": close}), "{name}");
            continue;
        }
        let UiInput::Create { doc, wait } = parsed else { panic!("{name}: not a create") };
        assert_eq!(serde_json::to_value(&doc).unwrap(), f["doc"], "{name}: normalized doc");
        let stored: CardDoc = serde_json::from_value(f["doc"].clone()).unwrap();
        assert_eq!(stored, doc, "{name}: the stored doc reads back");
        revalidate(&doc).unwrap_or_else(|e| panic!("{name}: {e}"));
        revalidate(&stored).unwrap_or_else(|e| panic!("{name}: stored: {e}"));
        if let Some(n) = f.get("input_bytes") {
            assert_eq!(json!(bytes(&f["input"])), *n, "{name}: input bytes");
            assert_eq!(json!(bytes(&f["doc"])), f["doc_bytes"], "{name}: doc bytes");
            assert!(bytes(&f["doc"]) <= DOC_MAX_BYTES, "{name}");
        }
        assert_eq!(json!(wait), f["wait"], "{name}");
        assert_eq!(serde_json::to_value(doc.mode()).unwrap(), f["mode"], "{name}");
        // The host's digest: a client only echoes it (§2.4), so its form is
        // all there is to check here.
        let contract = f["contract"].as_str().unwrap();
        assert!(
            contract
                .strip_prefix("c1:")
                .is_some_and(|h| h.len() == 32 && h.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())),
            "{name}: {contract}"
        );

        for a in f["answers"].as_array().unwrap() {
            let st = status(a["status"].as_str().unwrap());
            let values = if st == AnswerStatus::Skipped {
                assert_eq!(a["values"], json!({}), "{name}");
                Answer::skip()
            } else {
                parse_answer(&doc, &a["values"]).unwrap_or_else(|e| panic!("{name}: {e}"))
            };
            assert_eq!(serde_json::to_value(&values).unwrap(), a["values"], "{name}");
            let asked = a.get("late").map(|late| Capsule::parse(late["asked"].as_str().unwrap()).expect("the corpus's capsule"));
            let text = a["message"].as_str().unwrap();
            let back = parse_answer_message(text).unwrap_or_else(|| panic!("{name}: reads"));
            // `q` reads back as its attribute projection.
            assert_eq!(json!(back.q), a["q"], "{name}: q as an attribute");
            let m = AnswerMessage {
                card: a["card"].as_str().unwrap().into(),
                answer_id: a["answer_id"].as_str().unwrap().into(),
                q: back.q.clone(),
                status: st,
                rev: a["rev"].as_u64().unwrap() as u32,
                values: values.clone(),
                asked,
            };
            assert_eq!(back, m, "{name}: the host's message reads back as built");

            // As history: marked, it is the host's answer; the same text
            // without a mark (or with one that disagrees) is the user's.
            let mark = json!({"l": "9b1f", "card": m.card, "answer": m.answer_id, "rev": m.rev, "status": a["status"]});
            let folded = fold_answer(&user_text(text, Some(mark.clone()))).unwrap_or_else(|| panic!("{name}: a marked answer folds"));
            assert_eq!((&folded.card, &folded.answer_id, folded.status, folded.rev), (&m.card, &m.answer_id, st, m.rev), "{name}");
            assert_eq!((folded.late, &folded.values), (a.get("late").is_some(), &values), "{name}");
            assert_eq!(fold_answer(&user_text(text, None)), None, "{name}: a look-alike folds as typed text");
            let mut wrong = mark.clone();
            wrong["rev"] = json!(if m.rev == 4 { 3 } else { m.rev + 1 });
            assert_eq!(fold_answer(&user_text(text, Some(wrong))), None, "{name}: a mark that disagrees proves nothing");
        }
        for bad in f["bad_answers"].as_array().into_iter().flatten() {
            let e = parse_answer(&doc, &bad["values"]).expect_err(&name).to_string();
            assert_eq!(json!(e), bad["error"], "{name}");
            assert!(one_safe_line(&e), "{name}: {e:?}");
        }
        if let Some(r) = f.get("wait_result") {
            let a = &f["answers"][0];
            assert_eq!(r["status"], "answered");
            assert_eq!(r["answer_id"], a["answer_id"]);
            assert_eq!(r["id"], a["card"]);
            assert_eq!(r["answer"], a["values"], "{name}: a waited result carries the same answer object");
            let card = a["card"].as_str().unwrap();
            let answer_id = a["answer_id"].as_str().unwrap();
            let marked = json!({"l": "9b1f", "results": [{"card": card, "answer": answer_id}]});
            let fold = fold_history(&[assistant_call("toolu_w", &f["input"]), tool_result("toolu_w", r, Some(marked))]);
            let want = parse_answer(&doc, &a["values"]).unwrap();
            assert_eq!(
                fold.calls[0].outcome,
                Outcome::Answered { card: card.into(), answer_id: answer_id.into(), answer: want },
                "{name}: the waited result folds"
            );
            let fold = fold_history(&[assistant_call("toolu_w", &f["input"]), tool_result("toolu_w", r, None)]);
            assert_eq!(fold.calls[0].outcome, Outcome::NotRecorded, "{name}: unmarked, not proven");
        }
        if let Some(r) = f.get("shown") {
            let card = r["id"].as_str().unwrap();
            let marked = json!({"l": "9b1f", "results": [{"card": card, "answer": null}]});
            let fold = fold_history(&[assistant_call("toolu_s", &f["input"]), tool_result("toolu_s", r, Some(marked))]);
            assert_eq!(fold.calls[0].outcome, Outcome::Shown { card: card.into() }, "{name}: shown folds");
            let fold = fold_history(&[assistant_call("toolu_s", &f["input"]), tool_result("toolu_s", r, None)]);
            assert_eq!(fold.calls[0].outcome, Outcome::NotRecorded, "{name}: unmarked, not proven");
        }
    }
}

#[test]
fn invalid_fixtures() {
    let all = read_dir("invalid");
    assert!(all.len() >= 10);
    for (name, f) in all {
        let e = parse_input(&f["input"]).expect_err(&name).to_string();
        assert_eq!(json!(e), f["error"], "{name}");
        assert!(one_safe_line(&e), "{name}: {e:?}");
    }
}

/// The raw-JSON lane (Shady #439 L2): `decode_json`, then the model.
#[test]
fn raw_fixtures() {
    let all = read_dir("raw");
    assert!(all.len() >= 10);
    for (name, f) in all {
        let decoded = decode_json(f["text"].as_str().unwrap());
        let v = match (decoded, f["json_error"].as_str()) {
            (Err(e), Some(prefix)) => {
                assert!(e.to_string().starts_with(prefix), "{name}: {e}");
                assert!(one_safe_line(&e.to_string()), "{name}: {e:?}");
                continue;
            }
            (Ok(v), None) => v,
            (other, _) => panic!("{name}: {other:?}"),
        };
        match f["what"].as_str().unwrap() {
            "input" => match parse_input(&v) {
                Ok(UiInput::Create { doc, .. }) => assert_eq!(serde_json::to_value(&doc).unwrap(), f["doc"], "{name}"),
                Ok(other) => panic!("{name}: {other:?}"),
                Err(e) => assert_eq!(json!(e.to_string()), f["error"], "{name}"),
            },
            "answer" => {
                let Ok(UiInput::Create { doc, .. }) = parse_input(&f["card"]) else { panic!("{name}: card") };
                match parse_answer(&doc, &v) {
                    Ok(a) => assert_eq!(serde_json::to_value(&a).unwrap(), f["values"], "{name}"),
                    Err(e) => assert_eq!(json!(e.to_string()), f["error"], "{name}"),
                }
            }
            other => panic!("{name}: {other}"),
        }
    }
}

/// Scalar values assigned after the corpus's Unicode (16.0.0) whose full
/// case fold is not their lowercase, with the version that assigned them:
/// `casefold.json` can't list them. `fold_key` folds them as a toolchain
/// that knows them says (`CaseFolding.txt`); one that doesn't leaves them,
/// as the engine's does (its CI is Rust 1.95; the desk pins 1.100,
/// Unicode 18). Unicode 18's whole list: emulating `fold_key` on its UCD
/// matches its full folding at every scalar value.
const FOLDS_AFTER_THE_CORPUS: &[(u32, &str, (u8, u8, u8))] = &[
    // LATIN SMALL LIGATURE LONG S WITH DESCENDER S: `1DF95; F; 0073 0073`,
    // as U+00DF (its uppercase is `SS`).
    (0x1DF95, "ss", (18, 0, 0)),
];

/// The label comparison key is Unicode full case folding (Shady #439 M3),
/// pinned for every scalar value against Python's `str.casefold`, plus
/// [`FOLDS_AFTER_THE_CORPUS`] on a toolchain newer than the corpus.
#[test]
fn casefold_fixture() {
    let f = read("casefold.json");
    let table = f["differs_from_lowercase"].as_object().unwrap();
    assert!(table.len() > 200);
    assert_eq!(f["unicode"], "16.0.0", "FOLDS_AFTER_THE_CORPUS is relative to the corpus's Unicode");
    let (major, minor, update) = char::UNICODE_VERSION;
    let newer = |c: char| {
        FOLDS_AFTER_THE_CORPUS
            .iter()
            .find(|(cp, _, since)| *cp == c as u32 && char::UNICODE_VERSION >= *since)
            .map(|(_, f, _)| f.to_string())
    };
    let mut wrong = Vec::new();
    for c in (0..=0x10FFFFu32).filter_map(char::from_u32) {
        let expected = match table.get(&format!("U+{:04X}", c as u32)) {
            Some(v) => v.as_str().unwrap().to_string(),
            None => newer(c).unwrap_or_else(|| c.to_lowercase().collect()),
        };
        if fold_key(&c.to_string()) != expected {
            wrong.push(format!("U+{:04X}", c as u32));
        }
    }
    assert!(
        wrong.is_empty(),
        "fold_key differs from casefold.json (Unicode {}; this toolchain {major}.{minor}.{update}) at {wrong:?}",
        f["unicode"]
    );
}

#[test]
fn sanitize_fixtures() {
    let f = read("sanitize.json");
    let cases = f["cases"].as_array().unwrap();
    assert!(cases.len() >= 20);
    for c in cases {
        let s = c["in"].as_str().unwrap();
        assert_eq!(json!(ui_sanitize(s, Lines::Single)), c["single"], "{s:?}");
        assert_eq!(json!(ui_sanitize(s, Lines::Multi)), c["multi"], "{s:?}");
        let refused = refused_char(s).map(|(ch, why)| json!({"char": format!("U+{:04X}", ch as u32), "why": why}));
        assert_eq!(refused.unwrap_or(Value::Null), c["refused"], "{s:?}");
    }
}
