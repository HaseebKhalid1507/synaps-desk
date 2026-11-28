//! Settings' find (settings-overhaul PLAN v3.2 §1.7, §3.3, §3.6; PR 3).
//!
//! A query is at most 16 words in 256 bytes, held in a fixed buffer
//! ([`Query`]). A row's own strings are read in place, ASCII
//! case-insensitively, with the separators (`_ . - /` and space) skipped on
//! both sides: nothing is copied for search, and every hit is a byte range
//! of the string it is in.
//! - **Every word must hit** (AND).
//! - **Typos.** A word of 4 or more characters may be one edit (a
//!   substitution, an insertion, a deletion or two neighbours swapped, of
//!   characters) from a word of the label or the key, never of the help.
//!   A word is letters and digits; `—` ends one as a space does.
//! - **Exact first.** When some row has every word as typed (or by its
//!   alias), only such rows are found: `bash` finds the bash rows, not Dash
//!   URL. When none does, words may be one edit off, in any row: `content
//!   window` finds Context window ([`Search::end`]).
//! - **Aliases** ([`ALIASES`]): `colour` finds Accent and Theme, and so on.
//! - **Rank.** Label prefix, label word start, key (or inside a label
//!   word), alias or value, help; a typo just below its place. A row's rank
//!   is the sum of its words', lower first; the page keeps its order among
//!   equals.
//! - **Hits.** Every place a word reads as typed in a field, leftmost
//!   first and never overlapping, at most 8 a row (the label's first).
//! - A field is read to its first 65,535 bytes, cut back to a character: a
//!   hit always slices its field, and a word the cut ends inside is no
//!   typo.
//! - **Once a query.** A row is matched when the query changes or its
//!   strings do, never because the page draws again ([`Search`]).
//! - Until PR 8 an `@word` is an ordinary word.

use std::mem::size_of;

/// The query's bytes and words searched (PLAN §1.7).
pub(crate) const MAX_BYTES: usize = 256;
pub(crate) const MAX_WORDS: usize = 16;
/// The largest legitimate inventory (PLAN §3.6): 14 app, 11 session, 89
/// config and 9 connection rows, 64 providers and 128 plugins.
pub(crate) const MAX_ROWS: usize = 315;
/// A row's hits drawn, at most.
pub(crate) const HITS_PER_ROW: usize = 8;
pub(crate) const MAX_HITS: usize = MAX_ROWS * HITS_PER_ROW;
/// A word this long (in characters, separators aside) may be a typo.
const TYPO_FROM: usize = 4;

/// What the field says when the query was cut.
pub(crate) const WORDS_NOTE: &str = "only the first 16 words are searched";
pub(crate) const BYTES_NOTE: &str = "only the first 256 bytes are searched";

/// The aliases (PLAN §1.7): a word, and what it finds in a label or a key.
const ALIASES: &[(&str, &[&str])] = &[
    ("colour", &["accent", "theme"]),
    ("color", &["accent", "theme"]),
    ("font", &["text size"]),
    ("zoom", &["text size"]),
    ("size", &["text size"]),
    ("dark", &["theme"]),
    ("light", &["theme"]),
    ("ctx", &["context"]),
    ("effort", &["thinking"]),
    ("reasoning", &["thinking"]),
    ("retry", &["retries"]),
    ("notify", &["card notifications"]),
    ("sidebar", &["sessions rail"]),
    ("prompt", &["system prompt"]),
];

/// Where a query was cut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cut {
    None,
    Bytes,
    Words,
}

/// A query: its first 256 bytes (trimmed) and its first 16 words, as spans
/// of them.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Query {
    text: [u8; MAX_BYTES],
    len: u16,
    spans: [(u16, u16); MAX_WORDS],
    n: u8,
    cut: Cut,
}

// PLAN §3.6's 256 B and 16 spans of 4 B, plus the length, the count and
// the cut.
const _: () = assert!(size_of::<Query>() == MAX_BYTES + MAX_WORDS * 4 + 4);

impl Query {
    pub(crate) const EMPTY: Query = Query { text: [0; MAX_BYTES], len: 0, spans: [(0, 0); MAX_WORDS], n: 0, cut: Cut::None };

    /// `typed`, trimmed, its first 256 bytes (to a character) and their
    /// first 16 words. A word of separators alone is no word.
    pub(crate) fn parse(typed: &str) -> Query {
        let typed = typed.trim();
        let mut end = typed.len().min(MAX_BYTES);
        while !typed.is_char_boundary(end) {
            end -= 1;
        }
        let kept = &typed[..end];
        let mut q = Query::EMPTY;
        q.text[..end].copy_from_slice(kept.as_bytes());
        q.len = end as u16;
        if end < typed.len() {
            q.cut = Cut::Bytes;
        }
        for word in kept.split_whitespace() {
            if word.bytes().all(is_sep) {
                continue;
            }
            if usize::from(q.n) == MAX_WORDS {
                q.cut = Cut::Words;
                break;
            }
            let at = word.as_ptr() as usize - kept.as_ptr() as usize;
            q.spans[usize::from(q.n)] = (at as u16, (at + word.len()) as u16);
            q.n += 1;
        }
        q
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.n == 0
    }

    /// The text kept, as typed.
    pub(crate) fn text(&self) -> &str {
        std::str::from_utf8(&self.text[..usize::from(self.len)]).unwrap_or_default()
    }

    pub(crate) fn words(&self) -> impl Iterator<Item = &str> + '_ {
        (0..self.n).map(|i| std::str::from_utf8(self.word(i)).unwrap_or_default())
    }

    fn word(&self, i: u8) -> &[u8] {
        let (a, b) = self.spans[usize::from(i)];
        &self.text[usize::from(a)..usize::from(b)]
    }

    /// What the field says: the query was cut.
    pub(crate) fn note(&self) -> Option<&'static str> {
        match self.cut {
            Cut::None => None,
            Cut::Bytes => Some(BYTES_NOTE),
            Cut::Words => Some(WORDS_NOTE),
        }
    }

    /// The words that may be a typo (a bit each): 4 characters or more.
    fn typo_words(&self) -> u16 {
        (0..self.n).filter(|&i| chars(self.word(i)) >= TYPO_FROM).fold(0, |m, i| m | 1 << i)
    }
}

impl Default for Query {
    fn default() -> Self {
        Query::EMPTY
    }
}

impl std::fmt::Debug for Query {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Query")
            .field("text", &self.text())
            .field("words", &self.words().collect::<Vec<_>>())
            .field("cut", &self.cut)
            .finish()
    }
}

/// A row's strings, read where they are. `value` is a shown value (a
/// Connection fact's, a provider's); the page's rows have none yet.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Fields<'a> {
    pub label: &'a str,
    pub key: &'a str,
    pub help: &'a str,
    pub value: &'a str,
}

impl<'a> Fields<'a> {
    pub(crate) fn new(label: &'a str, key: &'a str, help: &'a str) -> Self {
        Fields { label, key, help, value: "" }
    }

    /// A fact: its label and its value.
    pub(crate) fn fact(label: &'a str, value: &'a str) -> Self {
        Fields { label, value, ..Fields::default() }
    }
}

/// A hit's field.
pub(crate) const LABEL: u8 = 0;
pub(crate) const KEY: u8 = 1;
pub(crate) const HELP: u8 = 2;
pub(crate) const VALUE: u8 = 3;

/// A hit: bytes `start..end` of field `field` of row `row`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Hit {
    pub row: u16,
    pub field: u8,
    pub start: u16,
    pub end: u16,
}

const _: () = assert!(size_of::<Hit>() == 8);

/// Lower ranks first.
pub(crate) type Rank = u16;

/// A row found: its number in the pass (its hits' key) and its rank.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Found {
    pub row: u16,
    pub rank: Rank,
}

const _: () = assert!(size_of::<Found>() == 4);

// A word's place, best first: tier × 2, and + 1 for a typo.
const AT_LABEL_PREFIX: u16 = 0;
const AT_LABEL_WORD: u16 = 2;
const AT_KEY: u16 = 4;
const AT_ALIAS: u16 = 6;
const AT_HELP: u16 = 8;

fn is_sep(b: u8) -> bool {
    matches!(b, b'_' | b'.' | b'-' | b'/' | b' ')
}

/// A letter or a digit: what a word is made of.
fn is_word(c: char) -> bool {
    c.is_alphanumeric()
}

/// A word starts at byte `s` of `h` (a character's start): no letter or
/// digit before it.
fn word_start(h: &[u8], s: usize) -> bool {
    let Some(mut p) = s.checked_sub(1) else { return true };
    while p > 0 && h[p] & 0xC0 == 0x80 {
        p -= 1;
    }
    !std::str::from_utf8(&h[p..s]).ok().and_then(|c| c.chars().next()).is_some_and(is_word)
}

/// Characters, separators aside.
fn chars(t: &[u8]) -> usize {
    t.iter().filter(|&&b| !is_sep(b) && b & 0xC0 != 0x80).count()
}

/// A field as searched: its first 65,535 bytes (a hit's `u16`), cut back
/// to a character.
fn hay(s: &str) -> &str {
    let mut end = s.len().min(usize::from(u16::MAX));
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// `t` read in `h` from `s` on, separators skipped on both sides: where it
/// ends.
fn reads_at(h: &[u8], s: usize, t: &[u8]) -> Option<usize> {
    let (mut i, mut j) = (s, 0);
    loop {
        while j < t.len() && is_sep(t[j]) {
            j += 1;
        }
        if j == t.len() {
            return Some(i);
        }
        if i > s {
            while i < h.len() && is_sep(h[i]) {
                i += 1;
            }
        }
        if i == h.len() || !h[i].eq_ignore_ascii_case(&t[j]) {
            return None;
        }
        i += 1;
        j += 1;
    }
}

/// Where `t` first reads in `h` from byte `from` on; `word`: the first at
/// a word's start, if any is (a label's place is its best).
fn place(h: &[u8], t: &[u8], word: bool, from: usize) -> Option<(usize, usize)> {
    let first = *t.iter().find(|&&b| !is_sep(b))?;
    let mut any = None;
    for s in from..h.len() {
        if !h[s].eq_ignore_ascii_case(&first) {
            continue;
        }
        let Some(e) = reads_at(h, s, t) else { continue };
        if !word || word_start(h, s) {
            return Some((s, e));
        }
        any = any.or(Some((s, e)));
    }
    any
}

/// `w` and `t` are at most one edit apart, counted in characters (ASCII
/// case folded): a substitution, an insertion, a deletion or two
/// neighbours swapped.
fn one_edit(w: &str, t: &str) -> bool {
    let eq = |a: &str, b: &str| a.eq_ignore_ascii_case(b);
    let m = t.chars().count();
    // `w`'s characters, counted no further than could be one edit.
    let n = w.chars().take(m + 2).count();
    if n.abs_diff(m) > 1 {
        return false;
    }
    let (mut i, mut j) = (0, 0);
    let (mut a, mut b) = (w.chars(), t.chars());
    loop {
        match (a.next(), b.next()) {
            (Some(x), Some(y)) if x.eq_ignore_ascii_case(&y) => (i, j) = (i + x.len_utf8(), j + y.len_utf8()),
            (Some(_), Some(_)) => break,
            _ => return true,
        }
    }
    let (w, t) = (&w[i..], &t[j..]);
    fn rest(s: &str) -> &str {
        s.chars().next().map_or(s, |c| &s[c.len_utf8()..])
    }
    match n.cmp(&m) {
        std::cmp::Ordering::Equal => {
            eq(rest(w), rest(t)) || {
                let (mut x, mut y) = (w.chars(), t.chars());
                match (x.next(), x.next(), y.next(), y.next()) {
                    (Some(w0), Some(w1), Some(t0), Some(t1)) => {
                        w0.eq_ignore_ascii_case(&t1) && w1.eq_ignore_ascii_case(&t0) && eq(rest(rest(w)), rest(rest(t)))
                    }
                    _ => false,
                }
            }
        }
        std::cmp::Ordering::Greater => eq(rest(w), t),
        std::cmp::Ordering::Less => eq(w, rest(t)),
    }
}

/// The first word of `field` (as searched, [`hay`]) one edit from `t`
/// (separators already out). A word the cut ends inside is no word.
fn typo_in(field: &str, t: &str) -> Option<(usize, usize)> {
    let h = hay(field);
    let cut = field[h.len()..].chars().next().is_some_and(is_word);
    let mut start = None;
    for (i, c) in h.char_indices().chain([(h.len(), ' ')]) {
        match (is_word(c), start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                start = None;
                if (i < h.len() || !cut) && one_edit(&h[s..i], t) {
                    return Some((s, i));
                }
            }
            _ => {}
        }
    }
    None
}

/// The alias `t` is, if it is one.
fn alias(t: &[u8]) -> Option<&'static [&'static str]> {
    ALIASES.iter().find(|(a, _)| reads_at(a.as_bytes(), 0, t) == Some(a.len()) && chars(t) == a.len()).map(|(_, to)| *to)
}

type Spot = Option<(u16, u16)>;

fn spot(r: Option<(usize, usize)>) -> Spot {
    r.map(|(s, e)| (s as u16, e as u16))
}

/// Word `t`'s best place in a row (`typo`: it may be one edit off), and
/// where it is in each field (label, key, help, value).
fn word_place(t: &[u8], f: &Fields, typo: bool) -> Option<(u16, [Spot; 4])> {
    let (label, key, help, value) = (hay(f.label).as_bytes(), hay(f.key).as_bytes(), hay(f.help).as_bytes(), hay(f.value).as_bytes());
    let mut at = [None; 4];
    let mut best = u16::MAX;
    if let Some((s, e)) = place(label, t, true, 0) {
        at[usize::from(LABEL)] = spot(Some((s, e)));
        best = if s == 0 {
            AT_LABEL_PREFIX
        } else if word_start(label, s) {
            AT_LABEL_WORD
        } else {
            AT_KEY
        };
    }
    if let Some(r) = place(key, t, false, 0) {
        at[usize::from(KEY)] = spot(Some(r));
        best = best.min(AT_KEY);
    }
    if let Some(r) = place(value, t, false, 0) {
        at[usize::from(VALUE)] = spot(Some(r));
        best = best.min(AT_ALIAS);
    }
    if best > AT_ALIAS
        && let Some(to) = alias(t)
    {
        for name in to {
            let n = name.as_bytes();
            if let Some(r) = place(label, n, true, 0) {
                at[usize::from(LABEL)] = spot(Some(r));
            } else if let Some(r) = place(key, n, false, 0) {
                at[usize::from(KEY)] = spot(Some(r));
            } else {
                continue;
            }
            best = best.min(AT_ALIAS);
            break;
        }
    }
    if let Some(r) = place(help, t, false, 0) {
        at[usize::from(HELP)] = spot(Some(r));
        best = best.min(AT_HELP);
    }
    if best == u16::MAX && typo {
        let mut packed = [0u8; MAX_BYTES];
        let mut n = 0;
        for &b in t.iter().filter(|&&b| !is_sep(b)) {
            packed[n] = b;
            n += 1;
        }
        // A word less its separators (ASCII) is still whole characters.
        let t = std::str::from_utf8(&packed[..n]).unwrap_or_default();
        if let Some((s, e)) = typo_in(f.label, t) {
            at[usize::from(LABEL)] = spot(Some((s, e)));
            best = (if s == 0 { AT_LABEL_PREFIX } else { AT_LABEL_WORD }) + 1;
        } else if let Some(r) = typo_in(f.key, t) {
            at[usize::from(KEY)] = spot(Some(r));
            best = AT_KEY + 1;
        }
    }
    (best != u16::MAX).then_some((best, at))
}

/// Row `f` against `q`: found when every word hits (words in `typos` may
/// be one edit off): its rank, whether a word was one edit off, and how
/// many hits it wrote to `out` (as row `row`; none when `out` is empty).
/// The hits: the label's, then the key's, the help's and the value's; in
/// each field every place a word reads as typed, from the field's start,
/// leftmost first and never overlapping (as `str::match_indices`), or a
/// word's alias or typo, at its place; at most `out.len()`.
fn find_row(q: &Query, typos: u16, f: &Fields, row: u16, out: &mut [Hit]) -> Option<(Rank, bool, u8)> {
    let mut rank: Rank = 0;
    let mut typo = false;
    let mut at = [[None; 4]; MAX_WORDS];
    for i in 0..q.n {
        let (best, spots) = word_place(q.word(i), f, typos & 1 << i != 0)?;
        rank += best;
        typo |= best & 1 == 1;
        at[usize::from(i)] = spots;
    }
    let words = usize::from(q.n);
    let mut n = 0;
    'fields: for field in [LABEL, KEY, HELP, VALUE] {
        if n == out.len() {
            break;
        }
        let h = hay([f.label, f.key, f.help, f.value][usize::from(field)]).as_bytes();
        // Each word's next place in the field, and whether it reads there
        // as typed (else it is its alias's or typo's place, taken once).
        let mut next = [None; MAX_WORDS];
        let mut typed = [false; MAX_WORDS];
        for i in 0..words {
            next[i] = place(h, q.word(i as u8), false, 0);
            typed[i] = next[i].is_some();
            if !typed[i] {
                next[i] = at[i][usize::from(field)].map(|(s, e)| (usize::from(s), usize::from(e)));
            }
        }
        while let Some(i) = (0..words).filter(|&i| next[i].is_some()).min_by_key(|&i| next[i]) {
            let Some((s, e)) = next[i] else { break };
            let hit = Hit { row, field, start: s as u16, end: e as u16 };
            if !out[..n].contains(&hit) {
                if n == out.len() {
                    break 'fields;
                }
                out[n] = hit;
                n += 1;
            }
            next[i] = if typed[i] { place(h, q.word(i as u8), false, e.max(s + 1)) } else { None };
        }
    }
    Some((rank, typo, n as u8))
}

/// One row by itself, against `q` (any word of 4 or more characters may
/// be one edit off): its rank, its hits pushed to `out` as row 0.
#[cfg(test)]
pub(crate) fn find(q: &Query, f: &Fields, out: &mut Vec<Hit>) -> Option<Rank> {
    let mut hits = [Hit::default(); HITS_PER_ROW];
    let (rank, _, n) = find_row(q, q.typo_words(), f, 0, &mut hits)?;
    out.extend_from_slice(&hits[..usize::from(n)]);
    Some(rank)
}

// The matcher's work and the walks over the rows, on this thread (a test's
// own): tests read them.
#[cfg(test)]
thread_local! {
    static MATCHED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static WALKS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn count(c: &'static std::thread::LocalKey<std::cell::Cell<u64>>) {
    c.with(|c| c.set(c.get() + 1));
}

/// Rows matched on this thread so far (a row's words against its strings).
#[cfg(test)]
pub(crate) fn matched() -> u64 {
    MATCHED.with(|c| c.get())
}

/// Walks over the rows begun on this thread so far.
#[cfg(test)]
pub(crate) fn walks() -> u64 {
    WALKS.with(|c| c.get())
}

/// What a row was matched as: a hash of its strings, where they are
/// (`'static`, [`Search::take`]) or what they say ([`Search::take_made`]),
/// and whether it is lit.
struct Stamp(u64);

impl Stamp {
    fn word(&mut self, w: u64) {
        self.0 = (self.0.rotate_left(5) ^ w).wrapping_mul(0x517c_c1b7_2722_0a95);
    }

    fn bytes(&mut self, b: &[u8]) {
        let (words, rest) = b.as_chunks::<8>();
        for &w in words {
            self.word(u64::from_le_bytes(w));
        }
        let mut tail = [0u8; 8];
        tail[..rest.len()].copy_from_slice(rest);
        self.word(u64::from_le_bytes(tail));
        self.word(b.len() as u64);
    }

    /// Strings that never change, by where they are and their length.
    fn at(f: &Fields<'static>, lit: bool) -> u64 {
        let mut s = Stamp(1 | u64::from(lit) << 1);
        for x in [f.label, f.key, f.help, f.value] {
            s.word(x.as_ptr().addr() as u64);
            s.word(x.len() as u64);
        }
        s.0
    }

    /// Strings made for this frame, by what they say.
    fn of(f: &Fields, lit: bool) -> u64 {
        let mut s = Stamp(4 | u64::from(lit) << 1);
        for x in [f.label, f.key, f.help, f.value] {
            s.bytes(x.as_bytes());
        }
        s.0
    }
}

/// What the search keeps of a row: what it was matched as, its rank
/// ([`NONE`]: not found), whether a word was one edit off, and its hits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Item {
    stamp: u64,
    rank: Rank,
    typo: bool,
    hits: u8,
}

const _: () = assert!(size_of::<Item>() == 16);

/// An item not found.
const NONE: Rank = Rank::MAX;

/// The page's search: its query, and what each row of the page's walk
/// over its rows was found as, with its hits. A row is matched when the
/// query changes or its strings do, never because the page draws again
/// (a scroll, a help opened, a resize, motion): a walk reads what was
/// found. Both lists are reserved once, at the first query, to their caps
/// (PLAN §3.6) and never grow past them: a row past [`MAX_ROWS`] is
/// matched each walk, found but neither kept nor lit.
///
/// **Exact first.** When some row has every word as typed (or by its
/// alias), only such rows are found; when none does, a word of 4 or more
/// characters may be one edit off, in any row ([`Search::end`]).
pub(crate) struct Search {
    query: Query,
    /// Only rows with every word as typed are found (else typos too).
    exact: bool,
    /// Rows past the cap this walk: found as typed (1), with a typo (2).
    past: u8,
    /// The walk's next row.
    next: u16,
    /// The last walk's rows, by their place in it.
    items: Vec<Item>,
    /// Their hits: row `r`'s from `r * HITS_PER_ROW`.
    hits: Vec<Hit>,
}

impl Default for Search {
    fn default() -> Self {
        Search { query: Query::EMPTY, exact: true, past: 0, next: 0, items: Vec::new(), hits: Vec::new() }
    }
}

impl Search {
    /// A new query: whether it changed. What was found is forgotten.
    pub(crate) fn set(&mut self, q: Query) -> bool {
        let changed = q != self.query;
        if changed {
            self.items.clear();
            self.exact = true;
        }
        self.query = q;
        changed
    }

    pub(crate) fn query(&self) -> &Query {
        &self.query
    }

    /// A walk over the page's rows begins: each row [`take`]n in the
    /// page's order (the same rows, whatever is found), then [`end`].
    ///
    /// [`take`]: Search::take
    /// [`end`]: Search::end
    pub(crate) fn begin(&mut self) {
        #[cfg(test)]
        count(&WALKS);
        if !self.query.is_empty() && self.hits.is_empty() {
            self.items.reserve_exact(MAX_ROWS);
            self.hits.reserve_exact(MAX_HITS);
            self.hits.resize(MAX_HITS, Hit::default());
        }
        (self.next, self.past) = (0, 0);
    }

    /// The walk is over: whether to walk again. A walk answers the last
    /// walk's way (exact rows only, or typos too); when it finds the other
    /// way is right now (rows found as typed came or went), the page walks
    /// again, which matches nothing.
    pub(crate) fn end(&mut self) -> bool {
        if self.query.is_empty() {
            return false;
        }
        self.items.truncate(usize::from(self.next));
        let found = |typo| self.items.iter().any(|i| i.rank != NONE && i.typo == typo);
        let exact = self.past & 1 != 0 || found(false) || (self.past & 2 == 0 && !found(true));
        let again = exact != self.exact;
        self.exact = exact;
        again
    }

    /// Row `f`, its strings `'static` (known by where they are): found
    /// (every row is, with no query) or not; its hits kept.
    pub(crate) fn take(&mut self, f: &Fields<'static>) -> Option<Found> {
        self.take_row(f, || Stamp::at(f, true), true)
    }

    /// [`Search::take`] for a row drawn with other strings than `f`: found
    /// or not, its hits not kept.
    pub(crate) fn take_unlit(&mut self, f: &Fields<'static>) -> Option<Found> {
        self.take_row(f, || Stamp::at(f, false), false)
    }

    /// [`Search::take`] for a row of strings made for this frame (known by
    /// what they say).
    pub(crate) fn take_made(&mut self, f: &Fields) -> Option<Found> {
        self.take_row(f, || Stamp::of(f, true), true)
    }

    fn take_row(&mut self, f: &Fields, stamp: impl FnOnce() -> u64, lit: bool) -> Option<Found> {
        if self.query.is_empty() {
            return Some(Found { row: u16::MAX, rank: 0 });
        }
        let row = self.next;
        self.next = self.next.saturating_add(1);
        let at = usize::from(row);
        let item = if at < MAX_ROWS {
            let stamp = stamp();
            match self.items.get(at) {
                Some(i) if i.stamp == stamp => *i,
                _ => {
                    let room = if lit { HITS_PER_ROW } else { 0 };
                    let item = Self::matched(&self.query, f, row, &mut self.hits[at * HITS_PER_ROW..][..room], stamp);
                    if at < self.items.len() {
                        self.items[at] = item;
                    } else {
                        self.items.push(item);
                    }
                    item
                }
            }
        } else {
            let item = Self::matched(&self.query, f, row, &mut [], 0);
            if item.rank != NONE {
                self.past |= if item.typo { 2 } else { 1 };
            }
            item
        };
        (item.rank != NONE && !(self.exact && item.typo)).then_some(Found { row, rank: item.rank })
    }

    fn matched(q: &Query, f: &Fields, row: u16, out: &mut [Hit], stamp: u64) -> Item {
        #[cfg(test)]
        count(&MATCHED);
        match find_row(q, q.typo_words(), f, row, out) {
            Some((rank, typo, hits)) => Item { stamp, rank, typo, hits },
            None => Item { stamp, rank: NONE, typo: false, hits: 0 },
        }
    }

    /// Each row of `rows` found, as the page takes them (walked again when
    /// [`Search::end`] says so): its index and rank.
    #[cfg(test)]
    pub(crate) fn found_in(&mut self, rows: &[Fields]) -> Vec<(usize, Rank)> {
        let walk = |s: &mut Search| {
            s.begin();
            let found = rows.iter().enumerate().filter_map(|(i, f)| s.take_made(f).map(|x| (i, x.rank))).collect();
            (found, s.end())
        };
        let (found, again) = walk(self);
        if again { walk(self).0 } else { found }
    }

    /// Every row of `rows`: what is found.
    #[cfg(test)]
    pub(crate) fn run(&mut self, rows: &[Fields]) -> Vec<Found> {
        self.found_in(rows);
        self.results()
    }

    /// The rows found in the last walk (to the cap), in their order.
    #[cfg(test)]
    pub(crate) fn results(&self) -> Vec<Found> {
        let found = |i: &&Item| i.rank != NONE && !(self.exact && i.typo);
        self.items.iter().enumerate().filter(|(_, i)| found(i)).map(|(r, i)| Found { row: r as u16, rank: i.rank }).collect()
    }

    /// Row `row`'s hits.
    pub(crate) fn hits_of(&self, row: u16) -> &[Hit] {
        let at = usize::from(row);
        match self.items.get(at) {
            Some(i) if i.rank != NONE => &self.hits[at * HITS_PER_ROW..][..usize::from(i.hits)],
            _ => &[],
        }
    }

    /// Bytes held (PLAN §3.6: `size_of` and every buffer's capacity).
    #[cfg(test)]
    pub(crate) fn retained(&self) -> usize {
        let Search { query: _, exact: _, past: _, next: _, items, hits } = self;
        size_of::<Self>() + items.capacity() * size_of::<Item>() + hits.capacity() * size_of::<Hit>()
    }

    #[cfg(test)]
    pub(crate) fn capacities(&self) -> (usize, usize) {
        (self.items.capacity(), self.hits.capacity())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings;

    /// A row of the test inventory, owned.
    #[derive(Clone)]
    struct Owned {
        label: String,
        key: String,
        help: String,
        value: String,
    }

    fn row(label: &str, key: &str, help: &str, value: &str) -> Owned {
        Owned { label: label.into(), key: key.into(), help: help.into(), value: value.into() }
    }

    fn fields(rows: &[Owned]) -> Vec<Fields<'_>> {
        rows.iter().map(|r| Fields { label: &r.label, key: &r.key, help: &r.help, value: &r.value }).collect()
    }

    /// THIS APP's rows, as today's page (and PR 6's three more) words them.
    const APP: [(&str, &str, &str); 14] = [
        ("Theme", "theme", "One of the Synaps TUI's themes. Following the Synaps config shows the theme its `theme =` names."),
        ("Accent", "accent", "Replaces the theme's “you” and focus colours; never a state."),
        ("Text size", "text_size", "The centre pane's text, in px (Ctrl+= and Ctrl+- step it)."),
        ("Motion", "motion", "Full, reduced (no movement, short fades) or off (nothing animates). Follows the system until you choose."),
        (
            "Card notifications",
            "notify.cards",
            "An OS notification when an agent waits on a card while the window, or its session, is not in front.",
        ),
        (
            "Remember card choices",
            "cards.remember",
            "When an agent asks the same card question again in the same folder, the card starts with your last choice highlighted.",
        ),
        ("Sessions rail", "rail_collapsed", "Shown or folded to its strip (Ctrl+B); its dragged width."),
        ("Right panel", "right_collapsed", "Shown or folded to its strip (Ctrl+Alt+B); its dragged width."),
        ("Daemon label", "daemon_label", "Names a remote daemon in headers, since its Welcome carries no host name."),
        ("Dash URL", "dash_url", "A remote daemon's synaps-dash, through a forwarded port on 127.0.0.1 (other hosts are refused)."),
        ("Usage range", "usage_range", "The range Usage shows when it opens."),
        ("Usage host", "usage_host", "The host Usage shows when it opens."),
        ("Split panes", "panes.split", "Up to nine panes side by side; read once at launch."),
        ("Often changed", "often", "The rows you change most, at the top of All."),
    ];

    /// The largest legitimate inventory (PLAN §3.6), 315 rows: THIS APP's
    /// 14, the session's 11, the config's 89 (#107's `ALL`), Connection's
    /// 9 facts, 64 providers and 128 plugins, their names at 64 bytes.
    fn inventory() -> Vec<Owned> {
        let mut rows: Vec<Owned> = APP.iter().map(|(l, k, h)| row(l, k, h, "")).collect();
        for key in settings::SESSION_KEYS {
            let d = settings::def(key).expect("a session key's def");
            rows.push(row(d.label, d.key, d.help, ""));
        }
        rows.push(row("System prompt", "system_prompt", "This session's system prompt (/prompt in the TUI).", ""));
        rows.push(row("Cache keep-warm", "keepwarm", "This session's keep-warm, as its status says: off, auto, or stop after <N>h.", ""));
        rows.extend(settings::ALL.iter().map(|d| row(d.label, d.key, d.help, "")));
        assert!(rows.iter().all(|r| r.label.len() <= 64), "names at most 64 bytes");
        for (l, v) in [
            ("Daemon", "local daemon · v0.13.0 · pid 48213 · ~/.synaps/synaps.sock"),
            ("Where", "this machine's registry has its pid and socket"),
            ("Version", "0.13.0"),
            ("Process", "48213"),
            ("Profile", "default"),
            ("Protocol", "7"),
            ("Socket", "/home/haseeb/.synaps/run/daemon-default.sock"),
            ("synaps-dash", "http://127.0.0.1:7340 · answers · serves pid 48213 (default) · from its URL file"),
            ("Telemetry sources", "otel · usage.jsonl · the daemon's own counters"),
        ] {
            rows.push(row(l, "", "", v));
        }
        let name64 = |s: String| format!("{s}{}", "-and-a-name-as-long-as-a-name-may-be").chars().take(64).collect::<String>();
        for i in 0..64 {
            let name = name64(format!("provider-{i:02}-openrouter-compatible-gateway"));
            rows.push(row(&name, "", "", "openai-compatible · from config.toml · key in the keyring"));
        }
        for i in 0..128 {
            let name = name64(format!("plugin-{i:03}-tools-for-the-repository"));
            let help =
                "Adds tools for the agent: reads the repository, runs its checks, and reports what changed, with a note per file touched.";
            rows.push(row(&name, "", help, "enabled · v1.4.2 · user"));
        }
        assert_eq!(rows.len(), MAX_ROWS, "the largest legitimate inventory");
        rows
    }

    fn q(s: &str) -> Query {
        Query::parse(s)
    }

    /// The keys found by `query` over `rows`, in rank order (the page's
    /// order among equals), as the page takes its rows.
    fn found(rows: &[Owned], query: &str) -> Vec<String> {
        let mut s = Search::default();
        s.set(q(query));
        let mut by = s.found_in(&fields(rows));
        by.sort_by_key(|&(_, r)| r);
        by.into_iter().map(|(i, _)| rows[i].key.clone()).collect()
    }

    /// A field's hits, as byte ranges (start, end), in order.
    fn ranges(hits: &[Hit], field: u8) -> Vec<(usize, usize)> {
        let mut r: Vec<_> = hits.iter().filter(|h| h.field == field).map(|h| (usize::from(h.start), usize::from(h.end))).collect();
        r.sort_unstable();
        r
    }

    /// `w`'s first half, to a character.
    fn half(w: &str) -> &str {
        w.char_indices().nth(w.chars().count().div_ceil(2)).map_or(w, |(i, _)| &w[..i])
    }

    fn one(f: Fields, query: &str) -> Option<(Rank, Vec<Hit>)> {
        let mut hits = Vec::new();
        find(&q(query), &f, &mut hits).map(|r| (r, hits))
    }

    /// The text of each hit in a field.
    fn said(f: &Fields, hits: &[Hit], field: u8) -> Vec<String> {
        let s = [f.label, f.key, f.help, f.value][usize::from(field)];
        hits.iter().filter(|h| h.field == field).map(|h| s[usize::from(h.start)..usize::from(h.end)].to_string()).collect()
    }

    #[test]
    fn separators_are_ignored() {
        let rows = inventory();
        let warm = found(&rows, "keep warm");
        assert!(warm.contains(&"cache_keepwarm".to_string()), "keep warm → cache_keepwarm: {warm:?}");
        let ttl = found(&rows, "cache-ttl");
        assert_eq!(ttl.first().map(String::as_str), Some("cache_ttl"), "cache-ttl → cache_ttl first: {ttl:?}");
        let f = Fields::new("Cache keep-warm", "cache_keepwarm", "");
        let (_, hits) = one(f, "keep warm").expect("found");
        assert_eq!(said(&f, &hits, LABEL), ["keep", "warm"], "the label's runs");
        assert_eq!(said(&f, &hits, KEY), ["keep", "warm"], "the key's runs");
        let (_, hits) = one(f, "cache-keep").expect("found");
        assert_eq!(said(&f, &hits, KEY), ["cache_keep"], "a run across a separator");
    }

    #[test]
    fn one_edit_in_words_of_four_or_more() {
        let rows = inventory();
        let thinking = found(&rows, "thinkng");
        assert!(thinking.contains(&"thinking".to_string()), "thinkng → Thinking: {thinking:?}");
        let timeout = found(&rows, "timout");
        assert!(timeout.contains(&"bash_timeout".to_string()), "timout → bash_timeout: {timeout:?}");
        assert!(found(&rows, "ttk").is_empty(), "a word of 3 is never a typo (ttk is one edit from ttl)");
        let f = Fields::new("Thinking", "thinking", "");
        for typo in ["thinkng", "thnking", "thinkingg", "htinking", "thinkinf"] {
            let (_, hits) = one(f, typo).unwrap_or_else(|| panic!("{typo}: one edit"));
            assert_eq!(said(&f, &hits, LABEL), ["Thinking"], "{typo}: the word is the run");
        }
        assert!(one(f, "thnkng").is_none(), "two edits");
    }

    #[test]
    fn help_never_typo_matches() {
        let f = Fields::new("Shell limit", "shell_limit", "The longest a command runs before its timeout.");
        assert!(one(f, "timeout").is_some(), "the help, as typed");
        assert!(one(f, "timout").is_none(), "the help, one edit off");
        assert!(one(Fields::fact("Socket", "the daemon's timeout"), "timout").is_none(), "nor a value");
    }

    #[test]
    fn every_token_must_hit() {
        let rows = inventory();
        assert_eq!(found(&rows, "cache ttl").first().map(String::as_str), Some("cache_ttl"));
        assert!(found(&rows, "cache zzqx").is_empty(), "one word misses: nothing");
        let f = Fields::new("Cache TTL", "cache_ttl", "Prompt-cache lifetime.");
        assert!(one(f, "cache ttl").is_some());
        assert!(one(f, "cache bash").is_none());
        assert!(one(f, "lifetime prompt ttl").is_some(), "any field, any order");
    }

    #[test]
    fn aliases_find_their_rows() {
        let rows = inventory();
        for (alias, key) in [
            ("colour", "accent"),
            ("color", "theme"),
            ("font", "text_size"),
            ("zoom", "text_size"),
            ("dark", "theme"),
            ("ctx", "context_window"),
            ("reasoning", "thinking"),
            ("retry", "api_retries"),
            ("notify", "notify.cards"),
            ("sidebar", "rail_collapsed"),
        ] {
            let keys = found(&rows, alias);
            assert!(keys.contains(&key.to_string()), "{alias} → {key}: {keys:?}");
        }
        let f = Fields::new("Accent", "accent", "Replaces the theme's focus colours.");
        let (_, hits) = one(f, "color").expect("found");
        assert_eq!(said(&f, &hits, LABEL), ["Accent"], "what the alias found is the run");
    }

    #[test]
    fn rank_label_prefix_first() {
        let rank = |f: Fields, w: &str| one(f, w).map(|(r, _)| r).unwrap_or_else(|| panic!("{w} in {f:?}"));
        let prefix = rank(Fields::new("Cache TTL", "ttl", ""), "cache");
        let word = rank(Fields::new("Prompt cache", "x", ""), "cache");
        let key = rank(Fields::new("Lifetime", "cache_x", ""), "cache");
        let alias = rank(Fields::new("Accent", "accent", ""), "colour");
        let value = rank(Fields::fact("Telemetry", "the cache"), "cache");
        let help = rank(Fields::new("Lifetime", "x", "The cache's lifetime."), "cache");
        assert!(prefix < word && word < key && key < alias && alias < help, "{prefix} {word} {key} {alias} {help}");
        assert_eq!(alias, value, "alias or value: one place");
        let typo = rank(Fields::new("Prompt cache", "x", ""), "cahce");
        assert!(word < typo && typo < key, "a typo just below its place: {word} {typo} {key}");
        let rows = vec![
            row("Lifetime", "x", "The cache's lifetime.", ""),
            row("Lifetime", "cache_x", "", ""),
            row("Prompt cache", "y", "", ""),
            row("Cache TTL", "cache_ttl", "", ""),
        ];
        assert_eq!(found(&rows, "cache"), ["cache_ttl", "y", "cache_x", "x"], "rows by rank");
    }

    #[test]
    fn hits_are_byte_ranges_of_the_original() {
        let f = Fields::new("Idle exit — after", "idle_exit_secs", "Stops the daemon — after this long → with no client.");
        let (_, hits) = one(f, "after client").expect("found");
        assert_eq!(said(&f, &hits, LABEL), ["after"]);
        assert_eq!(said(&f, &hits, HELP), ["after", "client"]);
        let (_, hits) = one(f, "idle-exit →").expect("found");
        assert_eq!(said(&f, &hits, KEY), ["idle_exit"]);
        assert_eq!(said(&f, &hits, HELP), ["→"], "a character past the ASCII, whole");
        for h in &hits {
            let s = [f.label, f.key, f.help][usize::from(h.field)];
            assert!(s.is_char_boundary(usize::from(h.start)) && s.is_char_boundary(usize::from(h.end)), "{h:?}");
        }
        // Three characters: no typo to hide the case.
        let f = Fields::new("Été", "x", "");
        assert_eq!(one(f, "éTé").map(|(_, h)| said(&f, &h, LABEL)), None, "ASCII case only: é is not É");
        assert_eq!(one(f, "ÉTé").map(|(_, h)| said(&f, &h, LABEL)), Some(vec!["Été".to_string()]));
    }

    #[test]
    fn query_caps_hold() {
        let paste: String = (0..1000).map(|i| format!("w{i} ")).collect();
        assert!(paste.len() >= 4096);
        let p = q(&paste);
        assert!(p.text().len() <= MAX_BYTES, "{} bytes kept", p.text().len());
        assert_eq!(p.words().count(), MAX_WORDS, "16 words searched");
        assert_eq!(p.words().collect::<Vec<_>>()[15], "w15");
        assert_eq!(p.note(), Some(WORDS_NOTE), "the note shown");
        let long = "é".repeat(2048);
        let p = q(&long);
        assert!(p.text().len() <= MAX_BYTES && p.text().chars().all(|c| c == 'é'), "cut to a character");
        assert_eq!((p.words().count(), p.note()), (1, Some(BYTES_NOTE)));
        let fine = q("  keep   warm ");
        assert_eq!((fine.text(), fine.words().collect::<Vec<_>>(), fine.note()), ("keep   warm", vec!["keep", "warm"], None));
        assert!(q(" - _ ").is_empty(), "separators alone are no word");
        let rows = inventory();
        let mut s = Search::default();
        s.set(q(&paste));
        assert!(s.run(&fields(&rows)).is_empty(), "w0 … w15: nothing has all 16");
    }

    #[test]
    fn hits_stay_within_their_reserved_capacity() {
        let mut rows = inventory();
        let f = fields(&rows);
        let mut s = Search::default();
        s.set(q("e a t o"));
        let n = s.run(&f).len();
        assert_eq!(s.capacities(), (MAX_ROWS, MAX_HITS), "reserved once, to the caps");
        assert!(n > 300, "the worst case finds nearly every row: {n}");
        let per: Vec<usize> = s.results().iter().map(|r| s.hits_of(r.row).len()).collect();
        assert!(per.iter().all(|&k| k <= HITS_PER_ROW), "at most 8 a row");
        assert!(per.iter().filter(|&&k| k == HITS_PER_ROW).count() > 200, "most rows at the cap");
        // Past the inventory: 85 more plugins.
        rows.extend((0..85).map(|i| row(&format!("plugin-x{i}"), "", "Reads, edits and tests: one more plugin, past the cap.", "")));
        let f = fields(&rows);
        let all = s.found_in(&f);
        assert!(all.len() > MAX_ROWS && all.iter().any(|&(i, _)| i >= MAX_ROWS), "rows past the cap still found: {}", all.len());
        let n = s.run(&f).len();
        assert!(n <= MAX_ROWS, "listed to the cap: {n}");
        assert_eq!(s.capacities(), (MAX_ROWS, MAX_HITS), "and no growth");
        let worst: String = "e a t o i n s r l c u d p m h g ".into();
        s.set(q(&worst));
        s.run(&f);
        assert_eq!(s.capacities(), (MAX_ROWS, MAX_HITS), "16 words: no growth");
    }

    #[test]
    fn search_holds_no_copy_of_the_rows() {
        let rows = inventory();
        let f = fields(&rows);
        let mut s = Search::default();
        let words: Vec<&str> = rows.iter().flat_map(|r| r.label.split(' ')).filter(|w| w.len() > 2).collect();
        for i in 0..1000 {
            let w = words[i % words.len()];
            let typed = match i % 4 {
                0 => half(w).to_string(),
                1 => w.to_string(),
                2 => format!("{w} {}", words[(i * 7) % words.len()]),
                _ => format!("{w}x"),
            };
            s.set(q(&typed));
            s.run(&f);
        }
        s.set(q("cache"));
        let n = s.run(&f).len();
        assert!(n > 0, "it found rows: {n}");
        assert_eq!(s.capacities(), (MAX_ROWS, MAX_HITS));
        assert_eq!(
            s.retained(),
            size_of::<Search>() + MAX_ROWS * size_of::<Item>() + MAX_HITS * size_of::<Hit>(),
            "the query, its items and hits"
        );
        assert!(size_of::<Item>() <= 32, "PLAN §3.6: an item is at most 32 B");
        assert!(size_of::<Search>() <= size_of::<Query>() + 64, "the query inline, nothing else of size");
    }

    #[test]
    fn every_place_a_word_reads_is_lit() {
        let f = Fields::new("Ticked", "x", "Nothing is ticked for you; Enter sends what's ticked.");
        let (_, hits) = one(f, "ticked").expect("found");
        assert_eq!(
            (said(&f, &hits, LABEL), said(&f, &hits, HELP)),
            (vec!["Ticked".to_string()], vec!["ticked".to_string(), "ticked".to_string()])
        );
        let help = "a ".repeat(20);
        let f = Fields::new("Area", "a", &help);
        let (_, hits) = one(f, "a").expect("found");
        assert_eq!(hits.len(), HITS_PER_ROW, "at most 8 a row, the label's first: {hits:?}");
        assert_eq!((said(&f, &hits, LABEL), said(&f, &hits, KEY)), (vec!["A".to_string(), "a".to_string()], vec!["a".to_string()]));
    }

    /// Not in PLAN v3.2's list: exact first. When some row has every word
    /// as typed (or by its alias), only such rows are found: what keeps the
    /// continuity suite's `bash` to the bash rows. When none does, a word
    /// of 4 or more may be one edit off, in any row.
    #[test]
    fn exact_results_come_first() {
        let rows =
            vec![row("Dash URL", "dash_url", "", ""), row("Bash timeout", "bash_timeout", "", ""), row("synaps-dash", "", "", "answers")];
        assert_eq!(found(&rows, "bash"), ["bash_timeout"], "not Dash URL, one edit off");
        assert_eq!(found(&rows, "dahs"), ["dash_url", ""], "no row has dahs: one edit");
        assert_eq!(found(&rows, "bash dahs"), ["dash_url", ""], "no row has both as typed: each may be one edit off");
    }

    /// A row with a word as typed but not the query takes no typo away
    /// from another row (review P2-2).
    #[test]
    fn a_typo_is_read_past_an_unrelated_row() {
        let def = |k: &str| settings::def(k).map(|d| row(d.label, d.key, d.help, "")).expect("a def");
        let mut rows = vec![def("compaction_exclude"), def("context_window")];
        assert!(rows[0].help.starts_with("Content "), "a row with content as typed");
        assert_eq!(found(&rows, "content window"), ["context_window"], "content, one edit from context");
        rows.push(row("Notes", "notes", "Content of every kind, as typed.", ""));
        assert_eq!(found(&rows, "content window"), ["context_window"], "a row more takes nothing away");
        let all = inventory();
        let has = |query: &str, key: &str| found(&all, query).iter().any(|k| k == key);
        assert!(has("content window", "context_window"), "the whole inventory");
        assert!(has("base timeout", "bash_timeout"), "not taken away by Request compression's base URL");
        assert!(has("bash timout", "bash_timeout"), "a word as typed and a typo");
        assert!(has("colour thme", "theme"), "an alias and a typo");
        assert!(has("bash", "bash_timeout") && !has("bash", "dash_url"), "exact first");
    }

    /// Every place a word reads in a field is lit, from the field's start:
    /// leftmost first, never overlapping (as `str::match_indices`), at most
    /// 8 a row. The best place only ranks (review P2-3).
    #[test]
    fn every_occurrence_is_lit_from_the_fields_start() {
        let lit = |label: &str, w: &str| one(Fields::new(label, "", ""), w).map(|(_, h)| ranges(&h, LABEL)).unwrap_or_default();
        assert_eq!(lit("Subagent timeout", "t"), [(7, 8), (9, 10), (15, 16)], "the t in Subagent too");
        assert_eq!(lit("xfoo foo", "foo"), [(1, 4), (5, 8)], "the one before the word start");
        assert_eq!(lit("xcatxcat cat", "cat"), [(1, 4), (5, 8), (9, 12)], "each before it");
        assert_eq!(lit("banana", "ana"), [(1, 4)], "never overlapping");
        assert_eq!(lit(&"t".repeat(12), "t"), (0..8).map(|i| (i, i + 1)).collect::<Vec<_>>(), "the first 8");
        let rank = |label: &str| one(Fields::new(label, "", ""), "foo").map(|(r, _)| r);
        assert_eq!(rank("xfoo foo"), rank("bar foo"), "ranked by its word start");
    }

    /// A field is read to its first 65,535 bytes (a hit's `u16`), cut back
    /// to a character: every hit slices the field. A word the cut ends
    /// inside is no typo; a word the cut leaves whole may be (review P2-4).
    #[test]
    fn the_field_cap_cuts_at_a_character() {
        const CAP: usize = u16::MAX as usize;
        let slices = |f: &str, hits: &[Hit]| hits.iter().all(|h| f.get(usize::from(h.start)..usize::from(h.end)).is_some());
        for c in ['é', '中', '𠀀'] {
            for k in 1..c.len_utf8() {
                // `c` begins `k` bytes before the cap: the cap is inside it.
                let at = CAP - k;
                let label = format!("{}caf{c} tail", " ".repeat(at - 3));
                assert!(!label.is_char_boundary(CAP));
                for w in ["caf", "caff", "cafe", "cafx"].map(String::from).into_iter().chain([format!("caf{c}"), c.to_string()]) {
                    if let Some((_, hits)) = one(Fields::new(&label, "", &label), &w) {
                        assert!(slices(&label, &hits), "{c} {k} bytes before the cap, {w}: {hits:?}");
                    }
                }
                let f = Fields::new(&label, "", "");
                assert_eq!(one(f, "caf").map(|(_, h)| ranges(&h, LABEL)), Some(vec![(at - 3, at)]), "{c}: as typed, before the cut");
                assert!(one(f, "caff").is_none(), "{c}: the word the cut ends inside is no typo");
                assert!(one(f, &c.to_string()).is_none(), "{c}: past the cut");
            }
        }
        let label = format!("{}cafe more", " ".repeat(CAP - 4));
        assert_eq!(
            one(Fields::new(&label, "", ""), "caff").map(|(_, h)| ranges(&h, LABEL)),
            Some(vec![(CAP - 4, CAP)]),
            "whole at the cut"
        );
    }

    /// One edit is one character's (a substitution, an insertion, a
    /// deletion or two neighbours swapped), past ASCII too; a word ends at
    /// a character that is neither a letter nor a digit, `—` too (review
    /// P3-1).
    #[test]
    fn one_edit_counts_characters() {
        let typo = |label: &str, w: &str| {
            let f = Fields::new(label, "", "");
            one(f, w).map(|(_, h)| said(&f, &h, LABEL))
        };
        let word = |w: &str| Some(vec![w.to_string()]);
        assert_eq!(typo("abcd", "abcé"), word("abcd"), "a substitution");
        assert_eq!(typo("abcd", "abcéd"), word("abcd"), "an insertion");
        assert_eq!(typo("abcéd", "abcd"), word("abcéd"), "a deletion");
        assert_eq!(typo("abéd", "abdé"), word("abéd"), "two neighbours swapped");
        assert_eq!(typo("naïve", "naive"), word("naïve"), "i for ï");
        assert_eq!(typo("abcd", "abéé"), None, "two edits");
        assert_eq!(typo("abcd", "ébcdé"), None, "two insertions");
        assert_eq!(typo("Read—timeout", "timout"), word("timeout"), "— ends a word");
        let rank = |label: &str| one(Fields::new(label, "", ""), "timeout").map(|(r, _)| r);
        assert_eq!(rank("Read—timeout"), rank("Read timeout"), "a word starts after —");
    }

    /// A row is matched once a query, and again only when its text
    /// changes: walking the same rows again matches none (review P2-1).
    #[test]
    fn a_row_is_matched_once_per_query_or_text() {
        let mut rows = inventory();
        let mut s = Search::default();
        s.set(q("thinkng"));
        let first = s.found_in(&fields(&rows));
        assert!(!first.is_empty(), "a typo, found");
        let m = matched();
        for _ in 0..3 {
            assert_eq!(s.found_in(&fields(&rows)), first, "the same rows found");
        }
        assert_eq!(matched(), m, "the same query over the same rows: no row matched again");
        let last = rows.len() - 1;
        rows[last].label = "plugin-thinking-tools".into();
        let again = s.found_in(&fields(&rows));
        assert_eq!(matched() - m, 1, "only the row whose text changed");
        assert!(again.iter().any(|&(i, _)| i == last), "found by its new text");
        let m = matched();
        s.set(q("thinking"));
        s.found_in(&fields(&rows));
        assert_eq!(matched() - m, rows.len() as u64, "a new query: each row once");
    }

    #[test]
    fn sentence_at_holds_the_hit() {
        let help = "First, e.g. this. Second one. Third.";
        assert_eq!(&help[settings::sentence_at(help, 0)], settings::first_sentence(help));
        assert_eq!(&help[settings::sentence_at(help, help.find("one").unwrap())], "Second one.");
        assert_eq!(&help[settings::sentence_at(help, help.find("Third").unwrap())], "Third.");
    }

    /// The matcher's release bench (PLAN §3.6): 1,000 queries over the
    /// largest legitimate inventory, each a new query's match over every
    /// row; p50, p99 and the slowest. The matcher only: the page's whole
    /// keystroke (match, sort and build) is `settings_find_keystroke_bench`
    /// (settings_find_tests.rs). On bella:
    /// `cargo test --release -p synaps-desktop -- --ignored settings_find_bench --nocapture`.
    #[test]
    #[ignore]
    fn settings_find_bench() {
        let rows = inventory();
        let f = fields(&rows);
        let words: Vec<&str> = rows.iter().flat_map(|r| r.label.split(' ')).collect();
        let mut queries: Vec<String> = Vec::with_capacity(1000);
        // Typed a key at a time, then typos, two words, aliases, misses and
        // the worst case.
        'typed: for w in words.iter().cycle() {
            for end in 1..=w.len() {
                if queries.len() == 700 {
                    break 'typed;
                }
                if w.is_char_boundary(end) {
                    queries.push(w[..end].to_lowercase());
                }
            }
        }
        for (i, w) in words.iter().cycle().take(150).enumerate() {
            let mut t: Vec<char> = w.chars().collect();
            if t.len() > 3 {
                t.swap(1, 2);
            }
            queries.push(if i % 2 == 0 { t.into_iter().collect() } else { format!("{w} {}", words[(i * 13) % words.len()]) });
        }
        for a in ALIASES.iter().map(|(a, _)| *a).cycle().take(100) {
            queries.push(a.into());
        }
        queries.extend((0..45).map(|i| format!("zzq{i}")));
        queries.extend((0..5).map(|_| "e a t o i n s r l c u d p m h g".to_string()));
        assert_eq!(queries.len(), 1000);
        let mut s = Search::default();
        let mut took: Vec<std::time::Duration> = Vec::with_capacity(1000);
        for t in &queries {
            let at = std::time::Instant::now();
            s.set(q(t));
            std::hint::black_box(s.run(&f).len());
            took.push(at.elapsed());
        }
        took.sort();
        let us = |d: std::time::Duration| d.as_secs_f64() * 1e6;
        let bytes: usize = f.iter().map(|r| r.label.len() + r.key.len() + r.help.len() + r.value.len()).sum();
        println!(
            "settings_find_bench: {} rows, {bytes} bytes; 1000 queries: p50 {:.1} µs, p99 {:.1} µs, max {:.1} µs",
            f.len(),
            us(took[499]),
            us(took[989]),
            us(took[999])
        );
    }
}
