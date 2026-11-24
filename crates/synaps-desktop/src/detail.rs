//! An agent card's detail as markdown-lite (synaps-cards `docs/design.md`
//! §5.2): **bold**, *italic* (or _italic_), `inline code`, and lists (`- `,
//! `* `, `+ ` or `1. ` at a line's start). Nothing else is markup: a link
//! shows as plain text, as typed (§5.4: nothing in a card is clicked
//! through), and a heading's `#`, a quote's `>` or a fence draw as they
//! were typed. A delimiter with no partner on its line is text.
//!
//! The detail is sanitized first (`crate::ask::shown`); this only reads it.

use std::ops::Range;

/// How a stretch of a line is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Look {
    pub(crate) bold: bool,
    pub(crate) italic: bool,
    pub(crate) code: bool,
}

/// One line of the detail: its list mark (`•`, or its number as typed)
/// and its depth, and its text with the stretches drawn otherwise than
/// plain, in order and apart.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Line {
    pub(crate) lead: Option<String>,
    pub(crate) depth: usize,
    pub(crate) text: String,
    pub(crate) looks: Vec<(Range<usize>, Look)>,
}

/// `detail`'s lines, read.
pub(crate) fn lines(detail: &str) -> Vec<Line> {
    detail
        .split('\n')
        .map(|raw| {
            let (lead, depth, body) = list_mark(raw);
            let mut line = Line { lead, depth, ..Line::default() };
            inline(body, Look::default(), &mut line);
            line
        })
        .collect()
}

/// A list item's mark, its depth (two spaces a level) and its text; any
/// other line as it is.
fn list_mark(raw: &str) -> (Option<String>, usize, &str) {
    let body = raw.trim_start_matches(' ');
    let depth = (raw.len() - body.len()) / 2;
    if let Some(rest) = ["- ", "* ", "+ "].iter().find_map(|m| body.strip_prefix(m)) {
        return (Some("•".into()), depth, rest);
    }
    let digits = body.bytes().take_while(u8::is_ascii_digit).count();
    if (1..=3).contains(&digits)
        && let Some(rest) = body[digits..].strip_prefix(". ").or_else(|| body[digits..].strip_prefix(") "))
    {
        return (Some(format!("{}.", &body[..digits])), depth, rest);
    }
    (None, 0, raw)
}

/// `s` read into `line` with `look` underneath: code first (nothing inside
/// it is markup), then bold, then italic; a delimiter opens only before a
/// non-space and closes only after one, and `_` only at a word's edge (a
/// `snake_case` name stays one).
fn inline(s: &str, look: Look, line: &mut Line) {
    let mut rest = s;
    let mut prev: Option<char> = None;
    while let Some(c) = rest.chars().next() {
        if c == '`'
            && let Some(end) = rest[1..].find('`').filter(|&e| e > 0)
        {
            push(line, &rest[1..1 + end], Look { code: true, ..look });
            rest = &rest[end + 2..];
            prev = Some('`');
            continue;
        }
        if let Some(inner) = rest.strip_prefix("**")
            && let Some(end) = closing(inner, "**")
        {
            inline(&inner[..end], Look { bold: true, ..look }, line);
            rest = &inner[end + 2..];
            prev = Some('*');
            continue;
        }
        if (c == '*' || (c == '_' && prev.is_none_or(|p| !p.is_alphanumeric())))
            && !rest[1..].starts_with(c)
            && let Some(end) = closing(&rest[1..], if c == '*' { "*" } else { "_" })
                .filter(|&e| c == '*' || rest[1 + e + 1..].chars().next().is_none_or(|n| !n.is_alphanumeric()))
        {
            inline(&rest[1..1 + end], Look { italic: true, ..look }, line);
            rest = &rest[end + 2..];
            prev = Some(c);
            continue;
        }
        push(line, &rest[..c.len_utf8()], look);
        rest = &rest[c.len_utf8()..];
        prev = Some(c);
    }
}

/// Where `d` closes a stretch that starts `inner`: the next `d` after a
/// non-space (and not part of a longer run of its character), with a
/// non-space right after the opening.
fn closing(inner: &str, d: &str) -> Option<usize> {
    if inner.starts_with(char::is_whitespace) {
        return None;
    }
    let ch = d.chars().next()?;
    let mut from = 0;
    while let Some(i) = inner[from..].find(d).map(|i| i + from) {
        let before = inner[..i].chars().next_back();
        let after = inner[i + d.len()..].chars().next();
        if i > 0 && before.is_some_and(|b| !b.is_whitespace()) && after != Some(ch) && (d.len() > 1 || before != Some(ch)) {
            return Some(i);
        }
        from = i + d.len();
    }
    None
}

/// `text` onto `line` drawn as `look`, a stretch of its own unless it
/// draws as the one before it.
fn push(line: &mut Line, text: &str, look: Look) {
    let start = line.text.len();
    line.text.push_str(text);
    if look == Look::default() {
        return;
    }
    match line.looks.last_mut() {
        Some((r, l)) if *l == look && r.end == start => r.end = line.text.len(),
        _ => line.looks.push((start..line.text.len(), look)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const B: Look = Look { bold: true, italic: false, code: false };
    const I: Look = Look { bold: false, italic: true, code: false };
    const C: Look = Look { bold: false, italic: false, code: true };

    /// `s`'s first line reads as `text`, with `looks` its drawn stretches.
    fn check(s: &str, text: &str, looks: &[(&str, Look)]) {
        let l = lines(s).remove(0);
        let got: Vec<(&str, Look)> = l.looks.iter().map(|(r, k)| (&l.text[r.clone()], *k)).collect();
        assert_eq!((l.text.as_str(), got.as_slice()), (text, looks), "{s}");
    }

    #[test]
    fn bold_italic_and_code() {
        check("It **locks** the table", "It locks the table", &[("locks", B)]);
        check("about *40 seconds*, _maybe_", "about 40 seconds, maybe", &[("40 seconds", I), ("maybe", I)]);
        check("run `make migrate` first", "run make migrate first", &[("make migrate", C)]);
        // Code is code: nothing in it is markup.
        check("`**not bold**`", "**not bold**", &[("**not bold**", C)]);
        // Italic inside bold.
        check("**a *b* c**", "a b c", &[("a ", B), ("b", Look { bold: true, italic: true, code: false }), (" c", B)]);
    }

    #[test]
    fn what_is_not_markup_stays_as_typed() {
        for s in [
            "snake_case_name and 2 * 3 * 4",
            "a ** b ** c",
            "an *unclosed one",
            "`unclosed code",
            "see [the docs](https://example.com/x)",
            "# not a heading",
            "> not a quote",
            "```",
            "**",
            "a __dunder__ b",
        ] {
            let l = lines(s).remove(0);
            assert_eq!((l.text.as_str(), l.looks.len(), l.lead.as_deref()), (s, 0, None), "{s}");
        }
    }

    #[test]
    fn lists() {
        let l = lines("Steps:\n- stop the writers\n  - drain the queue\n2. run it\n+ check `rows`");
        let leads: Vec<_> = l.iter().map(|l| (l.lead.as_deref(), l.depth, l.text.as_str())).collect();
        assert_eq!(
            leads,
            vec![
                (None, 0, "Steps:"),
                (Some("•"), 0, "stop the writers"),
                (Some("•"), 1, "drain the queue"),
                (Some("2."), 0, "run it"),
                (Some("•"), 0, "check rows"),
            ]
        );
        assert_eq!(l[4].looks, vec![(6..10, C)]);
        // Not a list: no space after the mark, or a number too long.
        for s in ["-1 degrees", "*emphasis*", "2026. was a year"] {
            assert_eq!(lines(s)[0].lead, None, "{s}");
        }
    }
}
