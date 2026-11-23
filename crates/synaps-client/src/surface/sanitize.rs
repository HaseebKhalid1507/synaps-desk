//! `ui_sanitize`: the one sanitizer for every string a card renders (§4.3),
//! mirrored from SynapsCLI's `agent_engine::ui::sanitize` (C1, `6e895d27`).
//! The daemon applies it at normalization and the desk again at display
//! (defense in depth); the corpus's `sanitize.json` pins the two equal.
//!
//! It is the engine's `sanitize_prompt_text` plus the rest of the
//! default-ignorable set the event renderer drops (`events::format`, the
//! set [`crate::events`] mirrors for `<event>` attributes): WJ,
//! BOM, soft hyphen, variation selectors, tag characters. ZWJ and ZWNJ are
//! kept, as the prompt sanitizer keeps them, for emoji and scripts.
//!
//! Line endings first (v3.2 #1): `\r\n` and a lone `\r` become `\n` before
//! anything else looks at the string. Then `\t` becomes a space, and a
//! single-line field folds `\n` to a space.
//!
//! Two uses, one table:
//! - **Validation** of what the agent wrote ([`refused_char`]): a C0/C1
//!   control (other than `\n`, `\t` and a line-ending `\r`) or a bidi
//!   embedding, override or isolate is refused with a teaching line, never
//!   rewritten.
//! - **Normalization and display** ([`ui_sanitize`]): everything else in
//!   the set is dropped. It is idempotent, and it never adds a byte.
//!
//! Display sanitizing never changes data sent to the model: answer values
//! are validated ([`super::answer::parse_answer`]), not rewritten. A client turns
//! CRLF into `\n` in values before it commits; a `\r` in a value is refused.

use std::borrow::Cow;

use crate::events::is_invisible;

pub use super::types::Lines;

/// `\r\n` and a lone `\r` as `\n`.
pub fn normalize_newlines(s: &str) -> Cow<'_, str> {
    if s.contains('\r') { Cow::Owned(s.replace("\r\n", "\n").replace('\r', "\n")) } else { Cow::Borrowed(s) }
}

/// The rendered form of `s`.
pub fn ui_sanitize(s: &str, lines: Lines) -> String {
    normalize_newlines(s).chars().filter_map(|c| sanitize_char(c, lines)).collect()
}

fn sanitize_char(c: char, lines: Lines) -> Option<char> {
    match c {
        '\n' if lines == Lines::Multi => Some('\n'),
        '\n' | '\t' => Some(' '),
        // ZWNJ and ZWJ: kept for scripts and emoji sequences.
        '\u{200C}' | '\u{200D}' => Some(c),
        c if c.is_control() => None,
        // Line and paragraph separators (the prompt sanitizer drops them).
        '\u{2028}' | '\u{2029}' => None,
        c if is_invisible(c) => None,
        c => Some(c),
    }
}

/// Why an agent-written string is refused, if it is: the first character
/// that can reorder or break what the user reads. The caller names the
/// field and the code point.
pub fn refused_char(s: &str) -> Option<(char, &'static str)> {
    s.chars().find_map(|c| refusal(c).map(|why| (c, why)))
}

fn refusal(c: char) -> Option<&'static str> {
    match c {
        '\n' | '\t' | '\r' => None,
        c if c.is_control() => Some("a control character"),
        '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' => Some("a bidi control"),
        _ => None,
    }
}

/// Whether a user-typed value holds a C0/C1 control (§3.3 #11). `\n` and
/// `\t` are allowed only where `lines` is `Multi` (`long` fields); `\r`
/// never is.
pub fn value_control(s: &str, lines: Lines) -> Option<char> {
    s.chars().find(|&c| match c {
        '\n' | '\t' => lines == Lines::Single,
        c => c.is_control(),
    })
}

/// Unicode scalar count: the unit of every `chars` bound in §4.4 and of
/// JSON Schema's `minLength` / `maxLength`.
pub fn char_len(s: &str) -> usize {
    s.chars().count()
}

/// `s` cut to at most `max` characters on a character boundary, ending in
/// `…` when it was cut.
pub fn cut_chars(s: &str, max: usize) -> String {
    if char_len(s) <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// The key labels are compared by: Unicode full case folding
/// (`CaseFolding.txt`, statuses C and F, as Python's `str.casefold`), on
/// the sanitized label. No other normalization (no NFC). Used for "labels
/// must differ (ignoring case)" and for matching `default` to a label, so
/// the match is never ambiguous (v3.2 #4, Shady #439 M3).
///
/// Built from the standard library's case tables, character by character:
/// lowercase, uppercase, lowercase, which is full case folding except for
/// U+0131 (dotless i, which folding keeps) and Cherokee (which folding maps
/// to uppercase). The corpus pins it against `str.casefold` for every
/// scalar value (`casefold.json`).
pub fn fold_key(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '\u{0131}' {
            out.push(c);
            continue;
        }
        for l in c.to_lowercase() {
            for u in l.to_uppercase() {
                for f in u.to_lowercase() {
                    if matches!(f, '\u{AB70}'..='\u{ABBF}' | '\u{13F8}'..='\u{13FD}') {
                        out.extend(f.to_uppercase());
                    } else {
                        out.push(f);
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Line endings first (v3.2 #1), then `\t`; a single-line field folds `\n`.
    #[test]
    fn line_endings_tabs_and_folds() {
        assert_eq!(ui_sanitize("a\r\nb\rc\td", Lines::Multi), "a\nb\nc d");
        assert_eq!(ui_sanitize("a\r\nb\rc\td", Lines::Single), "a b c d");
        assert_eq!(ui_sanitize("x\u{200D}y\u{200C}z\u{200B}\u{2028}\u{FEFF}", Lines::Single), "x\u{200D}y\u{200C}z");
    }

    #[test]
    fn refusals_name_the_first_bad_character() {
        assert_eq!(refused_char("ok\nfine\t\r"), None);
        assert_eq!(refused_char("a\u{1b}b\u{202E}"), Some(('\u{1b}', "a control character")));
        assert_eq!(refused_char("a\u{2067}b"), Some(('\u{2067}', "a bidi control")));
        assert_eq!(value_control("a\nb", Lines::Multi), None);
        assert_eq!(value_control("a\nb", Lines::Single), Some('\n'));
        assert_eq!(value_control("a\rb", Lines::Multi), Some('\r'));
    }

    #[test]
    fn cuts_and_folds() {
        assert_eq!(cut_chars("abcdef", 6), "abcdef");
        assert_eq!(cut_chars("abcdefg", 6), "abcde…");
        assert_eq!(fold_key("Straße"), "strasse");
        assert_eq!(fold_key("ΣΑΣ"), fold_key("σας"));
        assert_eq!(fold_key("\u{0131}"), "\u{0131}", "dotless i folds to itself");
    }
}
