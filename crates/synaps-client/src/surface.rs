//! Agent cards ("surfaces", catalog `synaps:card/1`): the desk's model of
//! agent-drawn UI, phase D1 of `agent-ui/architecture.md` v3.4 (§7).
//!
//! **Inert.** The client declares `capabilities.surfaces: 0`, so a daemon
//! sends it no card and no `surface_result` (`wire::surface_decodes`), and
//! nothing in the desktop draws one (that is D2). What lands here is the
//! model the later phases build on.
//!
//! A hand mirror of SynapsCLI's card model at `6e895d27` (#444's head, with
//! C1 #439 and C2 #440 under it); the desk takes no SynapsCLI crate:
//! - [`types`]: the data (`agent_core::core::ui_types`): the doc, answers,
//!   marks, and the card state the wire carries.
//! - [`sanitize`]: `ui_sanitize` (§4.3), the refusals, `fold_key`.
//! - [`doc`]: [`parse_input`] and [`revalidate`] (§5.1).
//! - [`answer`]: [`parse_answer`] (§3.3 #11), [`parse_answer_message`] (§3.6).
//! - [`fold`]: history folded by host marks, never by text (§2.5).
//!
//! What only the host does is not mirrored: the contract digest (a client
//! echoes `SurfaceState.contract`), building the `<ui_answer>` message and
//! its capsule, roster labels, the secret heuristic (D2 draws its
//! footnote), and the sidecar's own records.
//!
//! Parity is a fixture contract (§4.3): C1's corpus is copied unchanged to
//! `tests/fixtures/ui/`, and `surface/parity_tests.rs` runs this mirror over
//! all of it, as `tests/ui_card_model.rs` runs the engine.
//!
//! One difference the mirror absorbs: in the desktop's build this crate's
//! `serde_json::Map` keeps insertion order (gpui turns on `preserve_order`);
//! built alone, as in the engine, it is sorted. Where the engine names "the
//! first" key of an object, or quotes a JSON value in a refusal, this
//! module sorts first ([`first_key`], [`sorted_json`]), so every build
//! writes the engine's line.

pub mod answer;
pub mod doc;
pub mod fold;
pub mod sanitize;
pub mod types;

#[cfg(test)]
mod parity_tests;

pub use answer::{AnswerMessage, Capsule, parse_answer, parse_answer_message};
pub use doc::{UiInput, parse_input, revalidate};
pub use fold::{CallInput, Fold, FoldedAnswer, FoldedCall, Outcome, fold_answer, fold_history, mark_of};
pub use sanitize::ui_sanitize;
pub use types::*;

use sanitize::{Lines as L, cut_chars};

/// One card off the wire (a `surface` event's, an `Attached.surfaces`
/// entry), as SynapsCLI's `wire::lenient_card` reads it (v3.4 #4): a verb
/// this build doesn't know is one it doesn't implement, so it leaves
/// `offers` and the card stays; any other shape this build can't read (an
/// unknown field or phase, a missing one) is `None`. Then the doc must
/// revalidate ([`revalidate`]): the state type's own rule for a doc off the
/// wire, and what lets a renderer trust the doc's bounds. `None` is a bug
/// guard, never a planned path: the caller counts it, never fails a frame.
pub fn read_card(mut card: serde_json::Value) -> Option<SurfaceState> {
    if let Some(offers) = card.get_mut("offers").and_then(serde_json::Value::as_array_mut) {
        offers.retain(|v| serde_json::from_value::<Verb>(v.clone()).is_ok());
    }
    let card: SurfaceState = serde_json::from_value(card).ok()?;
    revalidate(&card.doc).is_ok().then_some(card)
}

/// `Attached.surfaces` ([`read_card`] each): the cards read, and how many
/// weren't. Absent or `null` is no cards; anything but a list is one
/// rejected, and the frame still decodes.
pub fn read_cards(raw: serde_json::Value) -> (Vec<SurfaceState>, usize) {
    match raw {
        serde_json::Value::Null => (Vec::new(), 0),
        serde_json::Value::Array(cards) => {
            let n = cards.len();
            let read: Vec<SurfaceState> = cards.into_iter().filter_map(read_card).collect();
            let rejected = n - read.len();
            (read, rejected)
        }
        _ => (Vec::new(), 1),
    }
}

/// The longest refusal path and message, in characters (Shady #439 M1).
pub const ERROR_PATH_MAX: usize = 80;
pub const ERROR_MESSAGE_MAX: usize = 240;

/// A refusal: the offending path and one teaching line. `Display` is the
/// tool error the agent reads, e.g.
/// `options: at most 9 (got 12); ask in plain text or narrow the choices`.
///
/// Built only by [`UiError::new`], which puts both parts through
/// `ui_sanitize` (single line) and bounds them, so no key or value the
/// agent wrote can carry a control, a bidi override or a newline into the
/// error, or make it long (§4.3). Untrusted keys are written with
/// [`key_path`] first, so they stay readable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiError {
    path: String,
    message: String,
}

impl UiError {
    pub(crate) fn new(path: impl AsRef<str>, message: impl AsRef<str>) -> Self {
        Self {
            path: cut_chars(&ui_sanitize(path.as_ref(), L::Single), ERROR_PATH_MAX),
            message: cut_chars(&ui_sanitize(message.as_ref(), L::Single), ERROR_MESSAGE_MAX),
        }
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for UiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}

impl std::error::Error for UiError {}

/// A key the agent or a client wrote, as a path segment: bare when it is a
/// plain identifier (`blocks`), else a JSON string with everything outside
/// printable ASCII escaped (`"\u001b[31m"`), cut to 24 characters first.
pub(crate) fn key_path(k: &str) -> String {
    let plain = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'-';
    if !k.is_empty() && k.len() <= 24 && k.bytes().all(plain) {
        return k.to_string();
    }
    let mut out = String::from("\"");
    let n = k.chars().count();
    for c in k.chars().take(24) {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            ' '..='~' => out.push(c),
            c => {
                let mut units = [0u16; 2];
                for u in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{u:04x}"));
                }
            }
        }
    }
    if n > 24 {
        out.push('…');
    }
    out.push('"');
    out
}

/// Whether `v` nests deeper than `limit`, scalars counted as one level.
/// The recursion stops at `limit`, so a value of any depth is safe to ask
/// about (Shady #439 M2).
pub(crate) fn deeper_than(v: &serde_json::Value, limit: usize) -> bool {
    use serde_json::Value;
    if limit == 0 {
        return true;
    }
    match v {
        Value::Array(a) => a.iter().any(|v| deeper_than(v, limit - 1)),
        Value::Object(o) => o.values().any(|v| deeper_than(v, limit - 1)),
        _ => false,
    }
}

/// The compact JSON size of `v` (a value already within a depth bound),
/// counted without building it; `None` once it passes `cap`.
pub(crate) fn json_len_within(v: &impl serde::Serialize, cap: usize) -> Option<usize> {
    struct Count {
        n: usize,
        cap: usize,
    }
    impl std::io::Write for Count {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.n += buf.len();
            if self.n > self.cap {
                return Err(std::io::Error::other("over the cap"));
            }
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut c = Count { n: 0, cap };
    serde_json::to_writer(&mut c, v).ok().map(|()| c.n)
}

/// The first key of `obj` that `pick` takes, in sorted order: the engine's
/// `Map` is a `BTreeMap` and iterates sorted, as this crate's does built
/// alone; in the desktop's build (`serde_json/preserve_order`, turned on by
/// gpui) it keeps insertion order. Every "first unknown key" refusal goes
/// through here, so every build names the engine's key.
pub(crate) fn first_key<'a>(
    obj: &'a serde_json::Map<String, serde_json::Value>,
    pick: impl Fn(&str, &serde_json::Value) -> bool,
) -> Option<&'a String> {
    obj.iter().filter(|(k, v)| pick(k, v)).map(|(k, _)| k).min()
}

/// `v` as compact JSON with every object's keys sorted, as the engine's
/// `Value::to_string` writes it (see [`first_key`]): for values quoted in
/// a refusal.
pub(crate) fn sorted_json(v: &serde_json::Value) -> String {
    fn sorted(v: &serde_json::Value) -> serde_json::Value {
        match v {
            serde_json::Value::Object(o) => {
                let mut keys: Vec<&String> = o.keys().collect();
                keys.sort();
                serde_json::Value::Object(keys.into_iter().map(|k| (k.clone(), sorted(&o[k]))).collect())
            }
            serde_json::Value::Array(a) => serde_json::Value::Array(a.iter().map(sorted).collect()),
            other => other.clone(),
        }
    }
    sorted(v).to_string()
}

/// Decode JSON text that will become a `ui` input or answer values: the
/// shared ingress (Shady #439 L2). Stricter than `serde_json::from_str`:
/// a repeated key at any level is refused, not resolved last-wins. Like
/// `serde_json` it refuses lone surrogates, numbers out of `f64` range and
/// nesting past 128; the model's own depth and byte caps apply after.
pub fn decode_json(text: &str) -> Result<serde_json::Value, UiError> {
    let mut de = serde_json::Deserializer::from_str(text);
    let v = <Strict as serde::Deserialize>::deserialize(&mut de)
        .and_then(|v| de.end().map(|()| v.0))
        .map_err(|e| UiError::new("json", e.to_string()))?;
    Ok(v)
}

/// A `Value` decoded with repeated keys refused.
struct Strict(serde_json::Value);

impl<'de> serde::Deserialize<'de> for Strict {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(StrictVisitor).map(Strict)
    }
}

struct StrictVisitor;

impl<'de> serde::de::Visitor<'de> for StrictVisitor {
    type Value = serde_json::Value;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("JSON")
    }
    fn visit_bool<E>(self, b: bool) -> Result<Self::Value, E> {
        Ok(b.into())
    }
    fn visit_i64<E>(self, n: i64) -> Result<Self::Value, E> {
        Ok(n.into())
    }
    fn visit_u64<E>(self, n: u64) -> Result<Self::Value, E> {
        Ok(n.into())
    }
    fn visit_f64<E: serde::de::Error>(self, n: f64) -> Result<Self::Value, E> {
        serde_json::Number::from_f64(n).map(Into::into).ok_or_else(|| E::custom("number out of range"))
    }
    fn visit_str<E>(self, s: &str) -> Result<Self::Value, E> {
        Ok(s.into())
    }
    fn visit_string<E>(self, s: String) -> Result<Self::Value, E> {
        Ok(s.into())
    }
    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(serde_json::Value::Null)
    }
    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut a: A) -> Result<Self::Value, A::Error> {
        let mut out = Vec::new();
        while let Some(Strict(v)) = a.next_element()? {
            out.push(v);
        }
        Ok(out.into())
    }
    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut m: A) -> Result<Self::Value, A::Error> {
        let mut out = serde_json::Map::new();
        while let Some(k) = m.next_key::<String>()? {
            let Strict(v) = m.next_value()?;
            if out.contains_key(&k) {
                return Err(serde::de::Error::custom(format!("the key {} is given twice", key_path(&k))));
            }
            out.insert(k, v);
        }
        Ok(out.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shady #439 M1: whatever a key holds, a refusal is one short line
    /// with no control, bidi control, invisible character or newline.
    #[test]
    fn errors_are_one_safe_bounded_line() {
        let hostile = "\u{1b}[31m\n\u{202E}forged\u{200B}\u{85}";
        assert_eq!(key_path(hostile), r#""\u001b[31m\u000a\u202eforged\u200b\u0085""#);
        assert_eq!(key_path("blocks"), "blocks");
        assert_eq!(key_path("wait-for"), "wait-for");
        assert_eq!(key_path(""), r#""""#);
        assert_eq!(key_path("a\"b\\c"), r#""a\"b\\c""#);
        assert_eq!(key_path("\u{1F600}"), r#""\ud83d\ude00""#);
        assert_eq!(key_path(&"k".repeat(25)), format!("\"{}…\"", "k".repeat(24)));
        let e = UiError::new(format!("x{hostile}"), format!("m{hostile}\r\n{}", "y".repeat(500)));
        let shown = e.to_string();
        assert!(safe_line(&shown), "{shown:?}");
        assert_eq!(e.path(), "x[31m forged");
        assert!(shown.chars().count() <= ERROR_PATH_MAX + 2 + ERROR_MESSAGE_MAX);
    }

    /// One line, bounded, nothing that can hide or reorder text.
    pub(crate) fn safe_line(s: &str) -> bool {
        s.chars().count() <= ERROR_PATH_MAX + 2 + ERROR_MESSAGE_MAX
            && s.chars().all(|c| !c.is_control() && !crate::events::is_invisible(c) && !matches!(c, '\u{2028}' | '\u{2029}'))
    }

    #[test]
    fn json_len_stops_at_the_cap() {
        let v = serde_json::json!({"a": "x".repeat(100)});
        let n = serde_json::to_vec(&v).unwrap().len();
        assert_eq!(json_len_within(&v, n), Some(n));
        assert_eq!(json_len_within(&v, n - 1), None);
    }

    /// The engine's maps are sorted; this crate's are sorted built alone and
    /// keep insertion order in the desktop's build (`preserve_order`). CI
    /// runs this in both; the helpers read either the engine's way.
    #[test]
    fn keys_are_read_in_the_engines_order() {
        let v: serde_json::Value = serde_json::from_str(r#"{"zeta":1,"beta":{"y":[{"b":2,"a":1}],"x":0},"alpha":2}"#).unwrap();
        let o = v.as_object().unwrap();
        let built = o.keys().next().map(String::as_str);
        assert!(matches!(built, Some("zeta" | "alpha")), "insertion order or sorted: {built:?}");
        assert_eq!(first_key(o, |_, _| true).map(String::as_str), Some("alpha"));
        assert_eq!(first_key(o, |k, _| k != "alpha").map(String::as_str), Some("beta"));
        assert_eq!(first_key(o, |_, v| v.is_object()).map(String::as_str), Some("beta"));
        assert_eq!(sorted_json(&v), r#"{"alpha":2,"beta":{"x":0,"y":[{"a":1,"b":2}]},"zeta":1}"#);
    }

    /// Shady #439 L2: the raw-JSON ingress.
    #[test]
    fn decode_json_is_strict() {
        assert_eq!(
            decode_json(r#"{"a":[1,{"b":null}],"c":"\ud83d\ude00"}"#).unwrap(),
            serde_json::json!({"a": [1, {"b": null}], "c": "\u{1F600}"})
        );
        for (text, want) in [
            (r#"{"a":1,"a":2}"#, "json: the key a is given twice"),
            (r#"{"x":{"b":1,"b":1}}"#, "json: the key b is given twice"),
            (r#"[{"\u001b":1,"\u001b":2}]"#, r#"json: the key "\u001b" is given twice"#),
        ] {
            assert!(decode_json(text).unwrap_err().to_string().starts_with(want), "{text}");
        }
        for bad in [r#""\ud83d""#, r#""\ude00""#, r#""\ud83dx""#, "1e400", "-1e400", "{} {}", "{\"a\":1,}"] {
            let e = decode_json(bad).expect_err(bad);
            assert!(e.to_string().starts_with("json: "), "{e}");
        }
        assert!(decode_json(&format!("{}1{}", "[".repeat(128), "]".repeat(128))).is_err());
        assert!(decode_json(&format!("{}1{}", "[".repeat(127), "]".repeat(127))).is_ok());
        // Big integers stay numbers (u64, then f64); the model refuses them by type.
        assert!(decode_json("18446744073709551616").unwrap().is_f64());
        assert!(decode_json("-9223372036854775809").unwrap().is_f64());
    }
}
