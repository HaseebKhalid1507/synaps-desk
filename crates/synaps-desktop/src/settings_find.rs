//! Settings' find (settings-overhaul PLAN v3.2 §1.7, §3.3, §3.6; PR 3).
//!
//! A query is at most 16 words in 256 bytes, held in a fixed buffer
//! ([`Query`]). A row's own strings are read in place, ASCII
//! case-insensitively, with the separators (`_ . - /` and space) skipped on
//! both sides: nothing is copied for search, and every hit is a byte range
//! of the string it is in.
//! - **Every word must hit** (AND).
//! - **Typos.** A word of 4 or more characters may be one edit (a
//!   substitution, an insertion, a deletion or two neighbours swapped) from
//!   a word of the label or the key, never of the help. Only a word that no
//!   row has as typed is read that way: `bash` finds the bash rows, not
//!   Dash URL ([`Search::begin`]'s scan).
//! - **Aliases** ([`ALIASES`]): `colour` finds Accent and Theme, and so on.
//! - **Rank.** Label prefix, label word start, key (or inside a label
//!   word), alias or value, help; a typo just below its place. A row's rank
//!   is the sum of its words', lower first; the page keeps its order among
//!   equals.
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b >= 0x80
}

/// Characters, separators aside.
fn chars(t: &[u8]) -> usize {
    t.iter().filter(|&&b| !is_sep(b) && b & 0xC0 != 0x80).count()
}

/// A field as searched: up to 65,535 bytes (a hit's `u16`).
fn hay(s: &str) -> &[u8] {
    &s.as_bytes()[..s.len().min(usize::from(u16::MAX))]
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
        if !word || s == 0 || !is_word(h[s - 1]) {
            return Some((s, e));
        }
        any = any.or(Some((s, e)));
    }
    any
}

/// `w` and `t` are at most one edit apart (ASCII case folded).
fn one_edit(w: &[u8], t: &[u8]) -> bool {
    let eq = |a: &[u8], b: &[u8]| a.eq_ignore_ascii_case(b);
    let (n, m) = (w.len(), t.len());
    if n.abs_diff(m) > 1 {
        return false;
    }
    let mut i = 0;
    while i < n.min(m) && w[i].eq_ignore_ascii_case(&t[i]) {
        i += 1;
    }
    if i == n.min(m) {
        return true;
    }
    match n.cmp(&m) {
        std::cmp::Ordering::Equal => {
            eq(&w[i + 1..], &t[i + 1..])
                || (i + 1 < n
                    && w[i].eq_ignore_ascii_case(&t[i + 1])
                    && w[i + 1].eq_ignore_ascii_case(&t[i])
                    && eq(&w[i + 2..], &t[i + 2..]))
        }
        std::cmp::Ordering::Greater => eq(&w[i + 1..], &t[i..]),
        std::cmp::Ordering::Less => eq(&w[i..], &t[i + 1..]),
    }
}

/// The first word of `h` one edit from `t` (separators already out).
fn typo_in(h: &[u8], t: &[u8]) -> Option<(usize, usize)> {
    let mut i = 0;
    while i < h.len() {
        if !is_word(h[i]) {
            i += 1;
            continue;
        }
        let s = i;
        while i < h.len() && is_word(h[i]) {
            i += 1;
        }
        if one_edit(&h[s..i], t) {
            return Some((s, i));
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
    let (label, key, help, value) = (hay(f.label), hay(f.key), hay(f.help), hay(f.value));
    let mut at = [None; 4];
    let mut best = u16::MAX;
    if let Some((s, e)) = place(label, t, true, 0) {
        at[usize::from(LABEL)] = spot(Some((s, e)));
        best = if s == 0 {
            AT_LABEL_PREFIX
        } else if !is_word(label[s - 1]) {
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
        let t = &packed[..n];
        if let Some((s, e)) = typo_in(label, t) {
            at[usize::from(LABEL)] = spot(Some((s, e)));
            best = (if s == 0 { AT_LABEL_PREFIX } else { AT_LABEL_WORD }) + 1;
        } else if let Some(r) = typo_in(key, t) {
            at[usize::from(KEY)] = spot(Some(r));
            best = AT_KEY + 1;
        }
    }
    (best != u16::MAX).then_some((best, at))
}

/// Row `f` against `q`: found when every word hits (words in `typos` may
/// be one edit off), its rank; its hits (at most 8, label's first) pushed
/// to `out` as row `row` when `lit`.
fn find_row(q: &Query, typos: u16, f: &Fields, row: u16, out: &mut Vec<Hit>, lit: bool) -> Option<Rank> {
    let mut rank: Rank = 0;
    let mut at = [[None; 4]; MAX_WORDS];
    for i in 0..q.n {
        let (best, spots) = word_place(q.word(i), f, typos & 1 << i != 0)?;
        rank += best;
        at[usize::from(i)] = spots;
    }
    if lit {
        // Label first: each word's place, then every other place it reads
        // as typed in the field.
        let from = out.len();
        let hays = [hay(f.label), hay(f.key), hay(f.help), hay(f.value)];
        'fields: for field in [LABEL, KEY, HELP, VALUE] {
            let h = hays[usize::from(field)];
            for (i, spots) in at[..usize::from(q.n)].iter().enumerate() {
                let mut next = spots[usize::from(field)].map(|(s, e)| (usize::from(s), usize::from(e)));
                while let Some((start, end)) = next {
                    if out.len() - from == HITS_PER_ROW {
                        break 'fields;
                    }
                    let hit = Hit { row, field, start: start as u16, end: end as u16 };
                    if !out[from..].contains(&hit) {
                        out.push(hit);
                    }
                    next = place(h, q.word(i as u8), false, end.max(start + 1));
                }
            }
        }
    }
    Some(rank)
}

/// One row by itself, against `q` (any word of 4 or more characters may
/// be one edit off): its rank, its hits pushed to `out` as row 0.
#[cfg(test)]
pub(crate) fn find(q: &Query, f: &Fields, out: &mut Vec<Hit>) -> Option<Rank> {
    find_row(q, q.typo_words(), f, 0, out, true)
}

/// The words a row has as typed (a bit each).
fn as_typed(q: &Query, f: &Fields) -> u16 {
    (0..q.n).filter(|&i| word_place(q.word(i), f, false).is_some()).fold(0, |m, i| m | 1 << i)
}

/// The page's search: its query, the rows found in a pass over the rows
/// ([`Search::results`]) and their hits. Both lists are reserved once, at
/// the first query, to their caps (PLAN §3.6) and never grow past them: a
/// row past [`MAX_ROWS`] is found but neither listed nor lit.
pub(crate) struct Search {
    query: Query,
    /// Words that may be one edit off, this pass.
    typos: u16,
    /// Words some row has as typed, in this pass's scan.
    seen: u16,
    scanning: bool,
    /// The next row's number.
    next: u16,
    results: Vec<Found>,
    hits: Vec<Hit>,
}

impl Default for Search {
    fn default() -> Self {
        Search { query: Query::EMPTY, typos: 0, seen: 0, scanning: false, next: 0, results: Vec::new(), hits: Vec::new() }
    }
}

impl Search {
    /// A new query: whether it changed.
    pub(crate) fn set(&mut self, q: Query) -> bool {
        let changed = q != self.query;
        self.query = q;
        changed
    }

    pub(crate) fn query(&self) -> &Query {
        &self.query
    }

    /// A pass over the rows begins. True: it scans first, each row [`take`]n
    /// once for what it has as typed (answering None), then [`scanned`],
    /// then each row again. A word no row has as typed may then be one
    /// edit off.
    ///
    /// [`take`]: Search::take
    /// [`scanned`]: Search::scanned
    pub(crate) fn begin(&mut self) -> bool {
        if !self.query.is_empty() && self.hits.capacity() == 0 {
            self.results.reserve_exact(MAX_ROWS);
            self.hits.reserve_exact(MAX_HITS);
        }
        self.results.clear();
        self.hits.clear();
        (self.next, self.seen, self.typos) = (0, 0, 0);
        self.scanning = self.query.typo_words() != 0;
        self.scanning
    }

    /// The scan is done.
    pub(crate) fn scanned(&mut self) {
        self.typos = self.query.typo_words() & !self.seen;
        (self.scanning, self.next) = (false, 0);
    }

    /// Row `f`: found (every row is, with no query) or not; its hits kept.
    pub(crate) fn take(&mut self, f: &Fields) -> Option<Found> {
        self.take_row(f, true)
    }

    /// [`Search::take`] for a row drawn with other strings than `f`: found
    /// or not, its hits not kept.
    pub(crate) fn take_unlit(&mut self, f: &Fields) -> Option<Found> {
        self.take_row(f, false)
    }

    fn take_row(&mut self, f: &Fields, lit: bool) -> Option<Found> {
        if self.query.is_empty() {
            return Some(Found { row: u16::MAX, rank: 0 });
        }
        if self.scanning {
            self.seen |= as_typed(&self.query, f);
            return None;
        }
        let room = self.results.len() < MAX_ROWS;
        let row = self.next;
        let rank = find_row(&self.query, self.typos, f, row, &mut self.hits, lit && room)?;
        self.next = self.next.saturating_add(1);
        let found = Found { row, rank };
        if room {
            self.results.push(found);
        }
        Some(found)
    }

    /// Every row of `rows`, scan and all: what is found.
    #[cfg(test)]
    pub(crate) fn run(&mut self, rows: &[Fields]) -> &[Found] {
        if self.begin() {
            for f in rows {
                self.take(f);
            }
            self.scanned();
        }
        for f in rows {
            self.take(f);
        }
        &self.results
    }

    /// The rows found in this pass, in their order.
    #[cfg(test)]
    pub(crate) fn results(&self) -> &[Found] {
        &self.results
    }

    /// Row `row`'s hits.
    pub(crate) fn hits_of(&self, row: u16) -> &[Hit] {
        let a = self.hits.partition_point(|h| h.row < row);
        let b = a + self.hits[a..].partition_point(|h| h.row == row);
        &self.hits[a..b]
    }

    /// Bytes held (PLAN §3.6: `size_of` and every buffer's capacity).
    #[cfg(test)]
    pub(crate) fn retained(&self) -> usize {
        let Search { query: _, typos: _, seen: _, scanning: _, next: _, results, hits } = self;
        size_of::<Self>() + results.capacity() * size_of::<Found>() + hits.capacity() * size_of::<Hit>()
    }

    #[cfg(test)]
    pub(crate) fn capacities(&self) -> (usize, usize) {
        (self.results.capacity(), self.hits.capacity())
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
    /// order among equals), as the page takes its rows: scan, then each.
    fn found(rows: &[Owned], query: &str) -> Vec<String> {
        let f = fields(rows);
        let mut s = Search::default();
        s.set(q(query));
        if s.begin() {
            for r in &f {
                s.take(r);
            }
            s.scanned();
        }
        let mut by: Vec<(Rank, usize)> = f.iter().enumerate().filter_map(|(i, r)| s.take(r).map(|x| (x.rank, i))).collect();
        by.sort_by_key(|&(r, _)| r);
        by.into_iter().map(|(_, i)| rows[i].key.clone()).collect()
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
        let n = s.run(&f).len();
        assert_eq!(n, MAX_ROWS, "listed to the cap");
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
            size_of::<Search>() + MAX_ROWS * size_of::<Found>() + MAX_HITS * size_of::<Hit>(),
            "the query, results and hits"
        );
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

    /// Not in PLAN v3.2's list: what keeps the continuity suite's `bash`
    /// to the bash rows.
    #[test]
    fn a_word_found_as_typed_is_not_read_as_a_typo() {
        let rows =
            vec![row("Dash URL", "dash_url", "", ""), row("Bash timeout", "bash_timeout", "", ""), row("synaps-dash", "", "", "answers")];
        assert_eq!(found(&rows, "bash"), ["bash_timeout"], "not Dash URL, one edit off");
        assert_eq!(found(&rows, "dahs"), ["dash_url", ""], "no row has dahs: one edit");
        assert_eq!(found(&rows, "bash dahs"), Vec::<String>::new(), "each word on its own: both must hit one row");
    }

    #[test]
    fn sentence_at_holds_the_hit() {
        let help = "First, e.g. this. Second one. Third.";
        assert_eq!(&help[settings::sentence_at(help, 0)], settings::first_sentence(help));
        assert_eq!(&help[settings::sentence_at(help, help.find("one").unwrap())], "Second one.");
        assert_eq!(&help[settings::sentence_at(help, help.find("Third").unwrap())], "Third.");
    }

    /// The release bench (PLAN §3.6): 1,000 queries over the largest
    /// legitimate inventory, each a keystroke's whole search (its scan
    /// too); p50, p99 and the slowest. On bella:
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
