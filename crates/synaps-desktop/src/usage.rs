//! Usage without GPUI: the telemetry store's answers (synaps-dash's
//! `/api/telemetry/*`, API v1, `~/Jawz/workspace/synaps-telemetry/API.md`)
//! and everything the view draws from them, built once per arrival
//! (`usage-design/spec.md` sections 3–5), never per frame.
//!
//! Honesty rules: `null` is `—`, never 0; a rate or percentile with no data
//! is a gap in its line, not a dip; sums draw 0. Money is API-equivalent at
//! list price. The `x.*` sums of the design's fixtures are not in API v1:
//! each section draws its v1 fallback and says what it cannot show.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde::Deserialize;

// ── API v1 bodies ─────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Summary {
    pub generated_at: Option<i64>,
    pub now: Now,
    pub all: Totals,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Now {
    pub spend_today: Option<f64>,
    pub burn_rate_per_hour: Option<f64>,
    pub requests_today: Option<u64>,
    pub hit_pct_1h: Option<f64>,
    pub ttft_p50_ms_1h: Option<f64>,
    pub total_p50_ms_1h: Option<f64>,
    pub live_sessions: Option<u64>,
    pub live_workers: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Totals {
    pub spend: Option<f64>,
    pub requests: Option<u64>,
    pub sessions: Option<u64>,
    pub first_ts: Option<i64>,
    pub last_ts: Option<i64>,
    pub tokens: TokenCounts,
    pub saved_by_cache: Option<f64>,
    pub saved_by_cache_net: Option<f64>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(default)]
pub struct TokenCounts {
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub cache_read: Option<u64>,
    pub cache_write: Option<u64>,
}

impl TokenCounts {
    pub fn sum(&self) -> u64 {
        [self.input, self.output, self.cache_read, self.cache_write].iter().map(|v| v.unwrap_or(0)).sum()
    }
}

/// `spend?bucket=day&group=…`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Spend {
    pub series: Vec<Series>,
    /// `[day, "session" | "request"]`: from that day on, the grain.
    pub grain: Vec<(String, String)>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Series {
    pub key: String,
    pub points: Vec<(String, Option<f64>)>,
}

impl Spend {
    /// The first day of request-level data.
    pub fn request_day(&self) -> Option<&str> {
        self.grain.iter().find(|(_, g)| g == "request").map(|(d, _)| d.as_str())
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Agents {
    pub agents: Vec<Agent>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Agent {
    pub name: String,
    pub runs: Option<u64>,
    pub requests: Option<u64>,
    pub spend: Option<f64>,
    pub models: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Cache {
    pub daily: Vec<CacheDay>,
    pub miss_reasons: Vec<Miss>,
    pub keepwarm: Keepwarm,
    pub releases: Vec<Release>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct CacheDay {
    pub day: String,
    pub hit_pct: Option<f64>,
    pub read: Option<u64>,
    pub write_5m: Option<u64>,
    pub write_1h: Option<u64>,
    pub write_unknown: Option<u64>,
    pub write: Option<u64>,
    pub fresh_input: Option<u64>,
    pub grain: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Miss {
    pub reason: Option<String>,
    pub count: u64,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Keepwarm {
    pub pings: Option<u64>,
    pub cost: Option<f64>,
    pub est_saved: Option<f64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Release {
    pub tag: String,
    pub ts: i64,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Latency {
    pub daily: Vec<LatencyDay>,
    pub split: Split,
    pub retries: Retries,
    /// `[prompt tokens, total ms]`.
    pub by_context: Vec<(Option<f64>, Option<f64>)>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct LatencyDay {
    pub day: String,
    pub ttft_p50: Option<f64>,
    pub ttft_p95: Option<f64>,
    pub total_p50: Option<f64>,
    pub total_p95: Option<f64>,
    pub n: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Split {
    pub headers_p50: Option<f64>,
    pub first_event_p50: Option<f64>,
    pub stream_p50: Option<f64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Retries {
    pub requests_with_retry: Option<u64>,
    pub rate_pct: Option<f64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Sessions {
    pub total: Option<u64>,
    pub rows: Vec<SessionRow>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct SessionRow {
    pub id: String,
    pub host: Option<String>,
    pub title: Option<String>,
    pub model: Option<String>,
    pub created: Option<i64>,
    pub requests: Option<u64>,
    pub spend: Option<f64>,
    pub workers: Option<u64>,
    pub grain: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Heatmap {
    /// `[weekday (Monday 0), hour, requests]`, New York time.
    pub cells: Vec<(usize, usize, u64)>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Health {
    pub stops: Vec<Stop>,
    pub refusal_retries: Option<u64>,
    pub tools_changed_misses: Option<u64>,
    pub ingest: Ingest,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Stop {
    pub stop: Option<String>,
    pub count: u64,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Ingest {
    pub last_run: Option<i64>,
    pub errors: Option<u64>,
    pub bad_lines: Option<u64>,
    pub lost_bytes: Option<u64>,
    pub last_error: Option<String>,
    pub sources: Vec<Source>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Source {
    pub host: String,
    pub kind: String,
    pub path: String,
    pub offset: Option<u64>,
    pub size: Option<u64>,
    pub files: Option<u64>,
}

/// One load: every endpoint for one host filter and range.
#[derive(Clone, Debug, Default)]
pub struct Raw {
    pub summary: Summary,
    pub spend_host: Spend,
    pub spend_model: Spend,
    pub spend_role: Spend,
    pub agents: Agents,
    pub cache: Cache,
    pub latency: Latency,
    pub sessions: Sessions,
    pub heatmap: Heatmap,
    pub health: Health,
    /// `summary?host=<h>` per host chip: its last data in the range.
    pub sources: Vec<(String, Option<i64>)>,
}

impl Raw {
    /// A design fixture (`usage-design/data/<host>.json`): every endpoint's
    /// body under its key.
    #[cfg(test)]
    pub fn from_fixture(v: &serde_json::Value) -> Result<Raw, String> {
        let get = |k: &str| v.get(k).cloned().unwrap_or(serde_json::Value::Null);
        macro_rules! de {
            ($k:literal) => {
                serde_json::from_value(get($k)).map_err(|e| format!("{}: {e}", $k))?
            };
        }
        Ok(Raw {
            summary: de!("summary"),
            spend_host: de!("spend_host"),
            spend_model: de!("spend_model"),
            spend_role: de!("spend_role"),
            agents: de!("agents"),
            cache: de!("cache"),
            latency: de!("latency"),
            sessions: de!("sessions_by_cost"),
            heatmap: de!("heatmap"),
            health: de!("health"),
            sources: Vec::new(),
        })
    }
}

// ── the selection ─────────────────────────────────────────────────────────

/// The range chips.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Range {
    D7,
    #[default]
    D30,
    D90,
    All,
}

impl Range {
    pub const ALL: [Range; 4] = [Range::D7, Range::D30, Range::D90, Range::All];

    pub fn key(self) -> &'static str {
        match self {
            Range::D7 => "7d",
            Range::D30 => "30d",
            Range::D90 => "90d",
            Range::All => "all",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Range::D7 => "7D",
            Range::D30 => "30D",
            Range::D90 => "90D",
            Range::All => "ALL",
        }
    }

    pub fn parse(s: &str) -> Option<Range> {
        Range::ALL.into_iter().find(|r| r.key().eq_ignore_ascii_case(s) || r.label().eq_ignore_ascii_case(s))
    }

    /// `from` (unix ms) at `now_ms`; `None`: all time.
    pub fn from(self, now_ms: i64) -> Option<i64> {
        let days = match self {
            Range::D7 => 7,
            Range::D30 => 30,
            Range::D90 => 90,
            Range::All => return None,
        };
        Some(now_ms - days * 86_400_000)
    }
}

/// The paths one load asks for (`host`: `None` is ALL). The host chips
/// come from the unfiltered `spend?group=host`.
pub fn paths(host: Option<&str>, from: Option<i64>) -> Vec<(&'static str, String)> {
    let q = |extra: &str, with_host: bool| {
        let mut p: Vec<String> = Vec::new();
        if !extra.is_empty() {
            p.push(extra.to_string());
        }
        if let (true, Some(h)) = (with_host, host) {
            p.push(format!("host={}", query_value(h)));
        }
        if let Some(f) = from {
            p.push(format!("from={f}"));
        }
        if p.is_empty() { String::new() } else { format!("?{}", p.join("&")) }
    };
    let t = "/api/telemetry";
    vec![
        ("summary", format!("{t}/summary{}", q("", true))),
        ("spend_host", format!("{t}/spend{}", q("bucket=day&group=host", false))),
        ("spend_model", format!("{t}/spend{}", q("bucket=day&group=model", true))),
        ("spend_role", format!("{t}/spend{}", q("bucket=day&group=role", true))),
        ("agents", format!("{t}/agents{}", q("", true))),
        ("cache", format!("{t}/cache{}", q("", true))),
        ("latency", format!("{t}/latency{}", q("", true))),
        ("sessions", format!("{t}/sessions{}", q("sort=cost&limit=25", true))),
        ("heatmap", format!("{t}/heatmap{}", q("", true))),
        ("health", format!("{t}/health{}", q("", true))),
    ]
}

/// The per-host `summary` for the source line.
pub fn source_path(host: &str, from: Option<i64>) -> String {
    let host = query_value(host);
    match from {
        Some(f) => format!("/api/telemetry/summary?host={host}&from={f}"),
        None => format!("/api/telemetry/summary?host={host}"),
    }
}

/// A query value, percent-encoded (all but `A-Z a-z 0-9 - . _ ~`).
pub fn query_value(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    for b in v.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The spend chart's switch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SpendBy {
    #[default]
    Host,
    Model,
    Role,
}

impl SpendBy {
    pub const ALL: [SpendBy; 3] = [SpendBy::Host, SpendBy::Model, SpendBy::Role];

    pub fn label(self) -> &'static str {
        match self {
            SpendBy::Host => "HOST",
            SpendBy::Model => "MODEL",
            SpendBy::Role => "MAIN / WORKER",
        }
    }
}

// ── formatting ────────────────────────────────────────────────────────────

pub const NONE: &str = "—";

/// `1234567` → `1,234,567`.
pub fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Money: `$4,689` from $1,000, `$522.89` below.
pub fn money(v: f64) -> String {
    if v.abs() >= 1000. {
        let sign = if v < 0. { "-" } else { "" };
        format!("{sign}${}", thousands(v.abs().round() as u64))
    } else {
        format!("${v:.2}")
    }
}

pub fn money_or(v: Option<f64>) -> String {
    v.map(money).unwrap_or_else(|| NONE.into())
}

pub fn count_or(v: Option<u64>) -> String {
    v.map(thousands).unwrap_or_else(|| NONE.into())
}

/// Three significant figures with K, M, B: `22.6B`, `9.28B`, `150M`, `812`.
pub fn tokens(n: u64) -> String {
    let n = n as f64;
    let (v, unit) = match n {
        n if n >= 1e9 => (n / 1e9, "B"),
        n if n >= 1e6 => (n / 1e6, "M"),
        n if n >= 1e3 => (n / 1e3, "K"),
        n => return format!("{n:.0}"),
    };
    format!("{}{unit}", sig3(v))
}

/// A size: `26.5 MB`, `812 B`.
pub fn bytes(n: u64) -> String {
    let t = tokens(n);
    match t.chars().last() {
        Some(u @ ('K' | 'M' | 'B')) => format!(
            "{} {}",
            &t[..t.len() - 1],
            if u == 'B' {
                "GB"
            } else if u == 'M' {
                "MB"
            } else {
                "KB"
            }
        ),
        _ => format!("{t} B"),
    }
}

/// Three significant figures (`1.15`, `32.8`, `150`).
fn sig3(v: f64) -> String {
    if v >= 99.95 {
        format!("{v:.0}")
    } else if v >= 9.995 {
        format!("{v:.1}")
    } else {
        format!("{v:.2}")
    }
}

/// Milliseconds as seconds, three significant figures: `1.15 s`.
pub fn secs(ms: f64) -> String {
    let s = ms / 1000.;
    if s < 1. { format!("{:.0} ms", ms) } else { format!("{} s", sig3(s)) }
}

pub fn secs_or(ms: Option<f64>) -> String {
    ms.map(secs).unwrap_or_else(|| NONE.into())
}

pub fn pct(v: f64) -> String {
    format!("{v:.1} %")
}

/// A local day (`2026-11-11`) as `Nov 11`.
pub fn day_label(day: &str) -> String {
    let mut it = day.split('-');
    let (_, m, d) = (it.next(), it.next().and_then(|m| m.parse::<usize>().ok()), it.next().and_then(|d| d.parse::<u32>().ok()));
    match (m, d) {
        (Some(m @ 1..=12), Some(d)) => format!("{} {d}", MONTHS[m - 1]),
        _ => day.to_string(),
    }
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// Unix ms as `Nov 14 05:32`, New York time (the store's days are New
/// York's; the desktop carries no time-zone database).
pub fn stamp(ms: i64) -> String {
    let t = ms / 1000 + ny_offset(ms / 1000);
    let (_, m, d) = civil(t.div_euclid(86_400));
    let s = t.rem_euclid(86_400);
    format!("{} {d} {:02}:{:02}", MONTHS[(m - 1) as usize], s / 3600, s % 3600 / 60)
}

/// A unix ms as its New York day, `2026-11-14`.
pub fn ny_day(ms: i64) -> String {
    let t = ms / 1000 + ny_offset(ms / 1000);
    let (y, m, d) = civil(t.div_euclid(86_400));
    format!("{y:04}-{m:02}-{d:02}")
}

/// America/New_York's offset at unix time `t` (US rules since 2007: EDT
/// from the second Sunday of March 07:00 UTC to the first Sunday of
/// November 06:00 UTC).
fn ny_offset(t: i64) -> i64 {
    let (y, _, _) = civil(t.div_euclid(86_400));
    let sunday = |m: u32, nth: i64| {
        let first = days_from_civil(y, m, 1);
        let dow = (first + 4).rem_euclid(7); // 0 = Sunday
        first + (7 - dow) % 7 + 7 * (nth - 1)
    };
    let start = sunday(3, 2) * 86_400 + 7 * 3600;
    let end = sunday(11, 1) * 86_400 + 6 * 3600;
    if (start..end).contains(&t) { -4 * 3600 } else { -5 * 3600 }
}

/// Days since 1970-01-01 → (year, month, day) (Hinnant's algorithm).
fn civil(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// How long ago `ms` was, at `now_ms`: `40 s`, `5 min`, `3 h`, `2 d`.
pub fn age(ms: i64, now_ms: i64) -> String {
    let s = ((now_ms - ms) / 1000).max(0);
    match s {
        s if s < 60 => format!("{s} s"),
        s if s < 3600 => format!("{} min", s / 60),
        s if s < 86_400 => format!("{} h", s / 3600),
        s => format!("{} d", s / 86_400),
    }
}

// ── what the view draws ───────────────────────────────────────────────────

/// A y axis: linear between `lo` and `hi`, or logarithmic (natural logs of
/// the bounds).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Scale {
    Linear(f64, f64),
    Log(f64, f64),
}

impl Scale {
    /// `v` as 0 (bottom) to 1 (top), clamped.
    pub fn at(self, v: f64) -> f32 {
        let (x, lo, hi) = match self {
            Scale::Linear(lo, hi) => (v, lo, hi),
            Scale::Log(lo, hi) => (v.max(1e-9).ln(), lo, hi),
        };
        if hi <= lo { 0. } else { ((x - lo) / (hi - lo)).clamp(0., 1.) as f32 }
    }

    /// The scale for these lines: from their lowest point to their highest,
    /// logarithmic.
    pub fn log_of(s: &Stack) -> Scale {
        let (lo, hi) = s
            .series
            .iter()
            .flat_map(|l| l.values.iter().flatten())
            .filter(|v| **v > 0.)
            .fold((f64::MAX, 0f64), |(lo, hi), &v| (lo.min(v), hi.max(v)));
        if hi <= 0. { Scale::Log(0., 1.) } else { Scale::Log((lo * 0.8).ln(), (hi * 1.1).ln()) }
    }

    fn bounds(self) -> (f64, f64) {
        match self {
            Scale::Linear(lo, hi) | Scale::Log(lo, hi) => (lo, hi),
        }
    }
}

/// A plot's axes, laid out once per load: its scale, the y ticks (value
/// and label) and the x ticks (day index and label).
#[derive(Clone, Debug)]
pub struct Axes {
    pub scale: Scale,
    pub y: Vec<(f64, String)>,
    pub x: Vec<(usize, String)>,
}

impl Default for Axes {
    fn default() -> Self {
        Axes { scale: Scale::Linear(0., 1.), y: Vec::new(), x: Vec::new() }
    }
}

impl Axes {
    /// From 0 to the stack's max, round ticks.
    fn linear(s: &Stack, fmt: fn(f64) -> String) -> Axes {
        Axes { scale: Scale::Linear(0., s.max), y: y_ticks(s.max, fmt), x: x_ticks(&s.days) }
    }

    /// A fixed floor and ceiling: three ticks.
    fn between(s: &Stack, lo: f64, hi: f64, fmt: fn(f64) -> String) -> Axes {
        let mid = (lo + hi) / 2.;
        Axes { scale: Scale::Linear(lo, hi), y: vec![(lo, fmt(lo)), (mid, fmt(mid)), (hi, fmt(hi))], x: x_ticks(&s.days) }
    }

    /// Logarithmic over the lines' points.
    fn log(s: &Stack, fmt: fn(f64) -> String) -> Axes {
        let scale = Scale::log_of(s);
        let (lo, hi) = scale.bounds();
        Axes { scale, y: log_ticks(lo, hi, fmt), x: x_ticks(&s.days) }
    }

    /// The y ticks as 0..1 of the plot (its grid lines).
    pub fn fracs(&self) -> Vec<f32> {
        self.y.iter().map(|(v, _)| self.scale.at(*v)).collect()
    }
}

/// A stacked daily chart: one value per day per series, `None` a gap.
#[derive(Clone, Debug, Default)]
pub struct Stack {
    pub days: Arc<[String]>,
    pub series: Vec<Line>,
    /// The tallest stack (bars) or the highest point (lines).
    pub max: f64,
}

#[derive(Clone, Debug, Default)]
pub struct Line {
    pub key: String,
    /// `s1`.. by [`crate::theme::Palette::series`].
    pub color: usize,
    pub values: Vec<Option<f64>>,
    pub total: f64,
    /// Its readout with no day under the pointer: the range total (or the
    /// last known day, for values that do not add up), formatted once.
    pub label: String,
}

impl Stack {
    /// Each line's readout label: its total, or (`latest`) its last day.
    fn labeled(mut self, fmt: fn(f64) -> String, latest: bool) -> Self {
        for l in &mut self.series {
            l.label =
                if latest { l.values.iter().rev().flatten().next().map_or_else(|| NONE.to_string(), |v| fmt(*v)) } else { fmt(l.total) };
        }
        self
    }

    fn finish_bars(mut self) -> Self {
        self.max = (0..self.days.len()).map(|i| self.series.iter().map(|s| s.values[i].unwrap_or(0.)).sum::<f64>()).fold(0., f64::max);
        self
    }

    /// The day index `x` (0..1 across the plot) falls on.
    pub fn index_at(&self, x: f32) -> Option<usize> {
        let n = self.days.len();
        (n > 0 && (0. ..=1.).contains(&x)).then(|| ((x * n as f32) as usize).min(n - 1))
    }
}

/// One headline cell.
#[derive(Clone, Debug, Default)]
pub struct Cell {
    pub label: &'static str,
    pub number: String,
    pub money: bool,
    pub second: String,
}

/// A host's line under the header.
#[derive(Clone, Debug)]
pub struct SourceLine {
    pub text: String,
    pub color: usize,
    pub empty: bool,
}

/// The hosts' fixed colours: avante `s1`, bella `s2`, jade `s3`, a fourth
/// `s4`, `unknown` the rest; other hosts after them by rank.
pub fn host_color(host: &str, rank: usize) -> usize {
    match host {
        "avante" => 0,
        "bella" => 1,
        "jade" => 2,
        "unknown" => 5,
        _ => (3 + rank).min(4),
    }
}

/// Everything the Usage view draws for one load.
#[derive(Clone, Debug, Default)]
pub struct Board {
    pub range: Range,
    pub generated_at: Option<i64>,
    pub hosts: Vec<String>,
    pub sources: Vec<SourceLine>,
    pub cells: Vec<Cell>,
    /// Host, model, main / worker, and their axes.
    pub spend: [Arc<Stack>; 3],
    pub spend_axes: [Axes; 3],
    /// The day request-level data starts (from the host-filtered series),
    /// as an index into each spend stack's own days.
    pub request_ix: [Option<usize>; 3],
    pub prompt: Arc<Stack>,
    pub prompt_axes: Axes,
    pub models: Vec<(String, f64, f64)>,
    pub roles: Vec<(String, f64)>,
    pub hit: Arc<Stack>,
    pub hit_axes: Axes,
    /// Days whose hit rate is under the plot's 50 % floor (drawn at it).
    pub hit_below: usize,
    pub legacy_until: Option<usize>,
    pub releases: Vec<(usize, String)>,
    pub saved: (Option<f64>, Option<f64>),
    pub writes: (u64, u64, u64),
    pub keepwarm: Keepwarm,
    pub misses: Vec<(String, u64)>,
    pub ttft: Arc<Stack>,
    pub total: Arc<Stack>,
    pub ttft_axes: Axes,
    pub total_axes: Axes,
    pub split: String,
    pub scatter: Arc<[(f32, f32)]>,
    /// Tokens across (x), total time up (y), both logarithmic.
    pub scatter_axes: (Axes, Axes),
    pub heat: Vec<[u64; 24]>,
    pub heat_max: u64,
    pub agents: Vec<Agent>,
    pub agents_total: f64,
    pub sessions: Vec<SessionRow>,
    pub sessions_total: Option<u64>,
    pub stops: String,
    pub health_line: String,
    pub ingest: Vec<Source>,
    pub ingest_line: String,
    /// What each section cannot show for this host.
    pub notes: Notes,
}

#[derive(Clone, Debug, Default)]
pub struct Notes {
    pub headline: String,
    pub spend: String,
    pub tokens: String,
    pub models: String,
    pub cache: String,
    pub latency: String,
    pub activity: String,
    pub agents: String,
    pub sessions: String,
}

/// The union of the days a chart's sources name, in order.
fn days_of<'a>(lists: impl Iterator<Item = &'a str>) -> Vec<String> {
    lists.collect::<BTreeSet<_>>().into_iter().map(str::to_string).collect()
}

/// Series on a shared day axis; the five largest kept, the rest summed
/// as `other` (when `top` is set).
fn stack_of(spend: &Spend, keep: impl Fn(&str) -> bool, top: Option<usize>, color: impl Fn(&str, usize) -> usize) -> Stack {
    // The axis is the kept series' own: a host chip's chart starts at that host's first day.
    let days = days_of(spend.series.iter().filter(|s| keep(&s.key)).flat_map(|s| s.points.iter().map(|p| p.0.as_str())));
    let ix: BTreeMap<&str, usize> = days.iter().enumerate().map(|(i, d)| (d.as_str(), i)).collect();
    let mut lines: Vec<Line> = spend
        .series
        .iter()
        .filter(|s| keep(&s.key))
        .map(|s| {
            let mut values = vec![None; days.len()];
            for (d, v) in &s.points {
                values[ix[d.as_str()]] = Some(v.unwrap_or(0.));
            }
            let total = values.iter().map(|v| v.unwrap_or(0.)).sum();
            Line { key: s.key.clone(), color: 0, values, total, label: String::new() }
        })
        .collect();
    lines.sort_by(|a, b| b.total.total_cmp(&a.total));
    if let Some(k) = top.filter(|&k| lines.len() > k) {
        let rest = lines.split_off(k);
        let mut values = vec![None; days.len()];
        for l in &rest {
            for (v, x) in values.iter_mut().zip(&l.values) {
                if let Some(x) = x {
                    *v = Some(v.unwrap_or(0.) + x);
                }
            }
        }
        lines.push(Line { key: "other".into(), color: 5, values, total: rest.iter().map(|l| l.total).sum(), label: String::new() });
    }
    for (rank, l) in lines.iter_mut().enumerate() {
        if l.key != "other" {
            l.color = color(&l.key, rank);
        }
    }
    Stack { days: days.into(), series: lines, max: 0. }.finish_bars()
}

/// Lines (not stacked) on one day axis; the max is the highest point.
fn lines_of(days: Vec<String>, series: Vec<Line>) -> Stack {
    let max = series.iter().flat_map(|s| s.values.iter().flatten().copied()).fold(0., f64::max);
    Stack { days: days.into(), series, max }
}

impl Board {
    /// Everything the view draws, from one load. `hosts`: the chips (the
    /// unfiltered host series, `unknown` never one).
    pub fn build(raw: &Raw, host: Option<&str>, range: Range, now_ms: i64) -> Board {
        let s = &raw.summary.all;
        let hosts: Vec<String> = {
            let mut h: Vec<&Series> = raw.spend_host.series.iter().filter(|s| s.key != "unknown").collect();
            h.sort_by(|a, b| a.key.cmp(&b.key));
            h.into_iter().map(|s| s.key.clone()).collect()
        };
        let rank = |k: &str| hosts.iter().position(|h| h == k).unwrap_or(0);
        // The spend charts.
        let by_host = stack_of(&raw.spend_host, |k| host.is_none_or(|h| h == k), None, |k, _| host_color(k, rank(k).saturating_sub(3)))
            .labeled(money, false);
        let by_model = stack_of(&raw.spend_model, |_| true, Some(5), |_, r| r.min(4)).labeled(money, false);
        let by_role = stack_of(&raw.spend_role, |_| true, None, |k, _| if k == "main" { 0 } else { 1 }).labeled(money, false);
        // The host-filtered series say where request-level data starts
        // (`spend_host` is unfiltered: the chips come from it).
        let request_day = raw.spend_model.request_day().or(raw.spend_role.request_day());
        let request_at = |s: &Stack| request_day.and_then(|d| s.days.iter().position(|x| x.as_str() >= d)).filter(|&i| i > 0);
        let request_ix = [request_at(&by_host), request_at(&by_model), request_at(&by_role)];
        let role_total = |k: &str| by_role.series.iter().find(|l| l.key == k).map(|l| l.total);
        // Cache.
        let cd = &raw.cache.daily;
        let sum = |f: fn(&CacheDay) -> Option<u64>| cd.iter().map(|d| f(d).unwrap_or(0)).sum::<u64>();
        let (read, write, fresh) = (sum(|d| d.read), sum(|d| d.write), sum(|d| d.fresh_input));
        let hit = (read + write + fresh > 0).then(|| 100. * read as f64 / (read + write + fresh) as f64);
        let cache_days: Vec<String> = cd.iter().map(|d| d.day.clone()).collect();
        let col = |key: &str, color: usize, f: fn(&CacheDay) -> Option<u64>| {
            let values: Vec<Option<f64>> = cd.iter().map(|d| Some(f(d).unwrap_or(0) as f64)).collect();
            Line { key: key.into(), color, total: values.iter().flatten().sum(), values, label: String::new() }
        };
        let prompt = Stack {
            days: cache_days.clone().into(),
            series: vec![col("cache read", 0, |d| d.read), col("cache write", 1, |d| d.write), col("fresh input", 2, |d| d.fresh_input)],
            max: 0.,
        }
        .finish_bars()
        .labeled(token_axis, false);
        let hit_line = Line {
            key: "hit rate".into(),
            color: 0,
            values: cd.iter().map(|d| d.hit_pct).collect(),
            total: hit.unwrap_or(0.),
            label: String::new(),
        };
        let hit_below = cd.iter().filter(|d| d.hit_pct.is_some_and(|v| v < HIT_FLOOR)).count();
        let legacy_until = cd.iter().rposition(|d| d.grain == "legacy").map(|i| i + 1);
        let releases = raw
            .cache
            .releases
            .iter()
            .filter_map(|r| {
                let day = ny_day(r.ts);
                let i = cache_days.iter().position(|d| *d >= day)?;
                (cache_days.first().is_some_and(|f| *f <= day)).then(|| (i, r.tag.clone()))
            })
            .collect();
        // Latency.
        let ld = &raw.latency.daily;
        let lat_days: Vec<String> = ld.iter().map(|d| d.day.clone()).collect();
        let lat = |key: &str, color: usize, f: fn(&LatencyDay) -> Option<f64>| {
            let values: Vec<Option<f64>> = ld.iter().map(|d| f(d).filter(|v| *v > 0.)).collect();
            Line { key: key.into(), color, total: 0., values, label: String::new() }
        };
        let ttft = lines_of(lat_days.clone(), vec![lat("p50", 0, |d| d.ttft_p50), lat("p95", 0, |d| d.ttft_p95)]).labeled(secs, true);
        let total = lines_of(lat_days, vec![lat("p50", 1, |d| d.total_p50), lat("p95", 1, |d| d.total_p95)]).labeled(secs, true);
        let sp = &raw.latency.split;
        let split = format!(
            "headers p50 {}  ·  first event p50 {}  ·  stream p50 {}  ·  retries {}",
            secs_or(sp.headers_p50),
            secs_or(sp.first_event_p50),
            secs_or(sp.stream_p50),
            match (raw.latency.retries.requests_with_retry, raw.latency.retries.rate_pct) {
                (Some(n), Some(r)) => format!("{} ({r:.1} %)", thousands(n)),
                (Some(n), None) => thousands(n),
                _ => NONE.into(),
            }
        );
        let pts: Vec<(f64, f64)> =
            raw.latency.by_context.iter().filter_map(|&(t, ms)| Some((t?, ms?))).filter(|(t, ms)| *t > 0. && *ms > 0.).collect();
        let lo_hi = |f: fn(&(f64, f64)) -> f64| {
            let (lo, hi) = pts.iter().map(f).fold((f64::MAX, 0f64), |(lo, hi), v| (lo.min(v), hi.max(v)));
            if pts.is_empty() { (1., 10.) } else { (lo.ln(), hi.ln().max(lo.ln() + 1e-6)) }
        };
        let (xr, yr) = (lo_hi(|p| p.0), lo_hi(|p| p.1));
        let scatter: Arc<[(f32, f32)]> =
            pts.iter().map(|(t, ms)| (((t.ln() - xr.0) / (xr.1 - xr.0)) as f32, ((ms.ln() - yr.0) / (yr.1 - yr.0)) as f32)).collect();
        // Activity.
        let mut heat = vec![[0u64; 24]; 7];
        for &(d, h, n) in &raw.heatmap.cells {
            if d < 7 && h < 24 {
                heat[d][h] = n;
            }
        }
        let heat_max = heat.iter().flatten().copied().max().unwrap_or(0);
        // Models (v1: spend only, from the model series).
        let model_total: f64 = raw.spend_model.series.iter().map(|s| s.points.iter().map(|p| p.1.unwrap_or(0.)).sum::<f64>()).sum();
        let mut models: Vec<(String, f64, f64)> = raw
            .spend_model
            .series
            .iter()
            .map(|s| {
                let t: f64 = s.points.iter().map(|p| p.1.unwrap_or(0.)).sum();
                (s.key.clone(), t, if model_total > 0. { t / model_total } else { 0. })
            })
            .filter(|m| m.1 > 0.)
            .collect();
        models.sort_by(|a, b| b.1.total_cmp(&a.1));
        let roles: Vec<(String, f64)> = ["main", "worker"]
            .iter()
            .map(|k| ((if *k == "main" { "main" } else { "workers" }).to_string(), role_total(k).unwrap_or(0.)))
            .collect();
        // Headline.
        let tokens_all = s.tokens.sum();
        let span = match (s.first_ts, s.last_ts) {
            (Some(a), Some(b)) => format!("{} to {}", day_label(&ny_day(a)), day_label(&ny_day(b))),
            _ => "no data in range".into(),
        };
        let now = &raw.summary.now;
        let cells = vec![
            Cell {
                label: "S P E N D",
                number: money_or(s.spend),
                money: true,
                second: format!("main {} · workers {}", money_or(role_total("main")), money_or(role_total("worker"))),
            },
            Cell {
                label: "R E Q U E S T S",
                number: count_or(s.requests),
                money: false,
                second: format!("{} today", count_or(now.requests_today)),
            },
            Cell { label: "S E S S I O N S", number: count_or(s.sessions), money: false, second: span },
            Cell {
                label: "T O K E N S",
                number: if s.requests.is_some() || tokens_all > 0 { tokens(tokens_all) } else { NONE.into() },
                money: false,
                second: format!(
                    "out {} · cached {}",
                    s.tokens.output.map(tokens).unwrap_or_else(|| NONE.into()),
                    s.tokens.cache_read.map(tokens).unwrap_or_else(|| NONE.into())
                ),
            },
            Cell {
                label: "C A C H E",
                number: hit.map(pct).unwrap_or_else(|| NONE.into()),
                money: false,
                second: match s.saved_by_cache_net {
                    Some(v) if hit.is_some() => format!("saved {} net", money(v)),
                    _ => format!("saved {NONE}"),
                },
            },
            Cell {
                label: "L A T E N C Y",
                number: secs_or(now.ttft_p50_ms_1h),
                money: false,
                second: format!("TTFT p50, last hour · total p50 {}", secs_or(now.total_p50_ms_1h)),
            },
        ];
        // The source line: each host chip's last data in the range.
        let sources = raw
            .sources
            .iter()
            .map(|(h, last)| {
                let (text, empty) = match last {
                    None => (format!("{h} no data in range"), true),
                    Some(t) if now_ms - t < 15 * 60_000 => (format!("{h} live"), false),
                    Some(t) => (format!("{h} to {}", stamp(*t)), false),
                };
                SourceLine { text, color: host_color(h, rank(h).saturating_sub(3)), empty }
            })
            .collect();
        // Agents, sessions, data.
        let agents_total = raw.agents.agents.iter().map(|a| a.spend.unwrap_or(0.)).sum();
        let h = &raw.health;
        let stops = h
            .stops
            .iter()
            .map(|s| format!("{} {}", s.stop.as_deref().unwrap_or("none"), thousands(s.count)))
            .collect::<Vec<_>>()
            .join("  ·  ");
        let health_line =
            format!("refusal retries {}  ·  tools-changed misses {}", count_or(h.refusal_retries), count_or(h.tools_changed_misses));
        let ingest_line = format!(
            "ingest {}  ·  errors {}  ·  bad lines {}  ·  lost bytes {}{}",
            h.ingest.last_run.map(|t| format!("{} ago", age(t, now_ms))).unwrap_or_else(|| NONE.into()),
            count_or(h.ingest.errors),
            count_or(h.ingest.bad_lines),
            count_or(h.ingest.lost_bytes),
            h.ingest.last_error.as_deref().map(|e| format!("  ·  last error: {e}")).unwrap_or_default()
        );
        let mut ingest: Vec<Source> = h.ingest.sources.iter().filter(|x| host.is_none_or(|hh| hh == x.host)).cloned().collect();
        ingest.sort_by(|a, b| (a.host.as_str(), a.kind.as_str()).cmp(&(b.host.as_str(), b.kind.as_str())));
        let notes = notes(host, raw, request_day);
        let hit_stack = lines_of(cache_days, vec![hit_line]).labeled(pct, false);
        let scatter_axes = (
            Axes { scale: Scale::Log(xr.0, xr.1), y: log_ticks(xr.0, xr.1, token_axis), x: Vec::new() },
            Axes { scale: Scale::Log(yr.0, yr.1), y: log_ticks(yr.0, yr.1, secs), x: Vec::new() },
        );
        Board {
            range,
            generated_at: raw.summary.generated_at,
            hosts,
            sources,
            cells,
            spend_axes: [Axes::linear(&by_host, money), Axes::linear(&by_model, money), Axes::linear(&by_role, money)],
            spend: [Arc::new(by_host), Arc::new(by_model), Arc::new(by_role)],
            request_ix,
            prompt_axes: Axes::linear(&prompt, token_axis),
            prompt: Arc::new(prompt),
            models,
            roles,
            hit_axes: Axes::between(&hit_stack, HIT_FLOOR, 100., |v| format!("{v:.0} %")),
            hit: Arc::new(hit_stack),
            hit_below,
            legacy_until,
            releases,
            saved: (s.saved_by_cache, s.saved_by_cache_net),
            writes: (sum(|d| d.write_5m), sum(|d| d.write_1h), sum(|d| d.write_unknown)),
            keepwarm: raw.cache.keepwarm.clone(),
            misses: raw.cache.miss_reasons.iter().map(|m| (m.reason.clone().unwrap_or_else(|| "unknown".into()), m.count)).collect(),
            ttft_axes: Axes::log(&ttft, secs),
            total_axes: Axes::log(&total, secs),
            ttft: Arc::new(ttft),
            total: Arc::new(total),
            split,
            scatter,
            scatter_axes,
            heat,
            heat_max,
            agents: raw.agents.agents.clone(),
            agents_total,
            sessions: raw.sessions.rows.clone(),
            sessions_total: raw.sessions.total,
            stops,
            health_line,
            ingest,
            ingest_line,
            notes,
        }
    }
}

/// The hit-rate plot's floor (%): days under it are drawn at it, and counted.
pub const HIT_FLOOR: f64 = 50.;

/// Tokens on an axis.
fn token_axis(v: f64) -> String {
    tokens(v as u64)
}

/// What each section cannot show (spec section 5), from what v1 says:
/// the grain, and which series are empty.
fn notes(host: Option<&str>, raw: &Raw, request_day: Option<&str>) -> Notes {
    let who = host.unwrap_or("these hosts");
    let grain = match request_day {
        Some(d) if raw.spend_model.grain.len() > 1 => {
            format!("Before {} ({who}), spend is session totals on the day each session began, all counted as main.", day_label(d))
        }
        None if !raw.spend_model.series.is_empty() => format!("{who}: session totals only, no request-level data."),
        _ => String::new(),
    };
    let no_ttft = raw.latency.daily.iter().all(|d| d.ttft_p50.is_none());
    let unknown = host == Some("unknown");
    Notes {
        headline: "API-equivalent at list price. Compaction calls are not in the api-log, so request-level spend undercounts a session's own total. \
                   TOKENS is session totals, where sessions from before cache totals were kept count no cache reads; the prompt chart below is \
                   request-level, so the two differ."
            .into(),
        spend: grain,
        tokens: "Output tokens per day need the store's daily totals (not in API v1): prompt tokens only.".into(),
        models: "Requests, tokens, hit rate and latency per model need the store's totals endpoint (not in API v1): spend only.".into(),
        cache: if unknown || raw.cache.daily.is_empty() {
            format!("{who}: no request-level cache data.")
        } else if raw.cache.keepwarm.pings.is_none() {
            format!("{who}: no keep-warm or miss-reason data.")
        } else {
            "Hit rate from cache.daily: Σ read / Σ (read + write + fresh input).".into()
        },
        latency: if raw.latency.daily.is_empty() {
            format!("{who}: no request-level latency.")
        } else if no_ttft {
            format!("{who}: no time-to-first-token samples; totals only.")
        } else {
            "TTFT is sampled: older api-log rows carry no first-event time, so its days have fewer samples than requests.".into()
        },
        activity: "Requests and sessions per day need the store's daily totals (not in API v1): the hour grid only.".into(),
        agents: "Runs count worker archives as stored: avante's copies of bella's archives count twice (329 runs when measured on 2026-11-14)."
            .into(),
        sessions: "MAIN CONVERSATION is session_cost alone; a session's workers are not in it. Sessions belong to the host that began them."
            .into(),
    }
}

/// A short host chip label, tracked caps (`A V A N T E`).
pub fn tracked(s: &str) -> String {
    let up = s.to_uppercase();
    let mut out = String::with_capacity(up.len() * 2);
    for (i, c) in up.chars().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

/// Month-start ticks (week starts under 45 days): `(index, label)`.
pub fn x_ticks(days: &[String]) -> Vec<(usize, String)> {
    let weekly = days.len() < 45;
    let mut out = Vec::new();
    for (i, d) in days.iter().enumerate() {
        let start = if weekly {
            let mut it = d.split('-').filter_map(|x| x.parse::<i64>().ok());
            match (it.next(), it.next(), it.next()) {
                (Some(y), Some(m), Some(dd)) => (days_from_civil(y, m as u32, dd as u32) + 3).rem_euclid(7) == 0,
                _ => false,
            }
        } else {
            d.ends_with("-01")
        };
        if start && i > 0 {
            out.push((i, if weekly { day_label(d) } else { day_label(d).split(' ').next().unwrap_or("").to_string() }));
        }
    }
    out
}

/// At most four y ticks from 0 to `max`, on round steps: `(value, label)`.
pub fn y_ticks(max: f64, label: impl Fn(f64) -> String) -> Vec<(f64, String)> {
    if max <= 0. {
        return Vec::new();
    }
    let raw = max / 3.;
    let mag = 10f64.powf(raw.log10().floor());
    let step = [1., 2., 2.5, 5., 10.].iter().map(|m| m * mag).find(|s| max / s <= 4.).unwrap_or(10. * mag);
    (1..=4).map(|i| i as f64 * step).filter(|v| *v <= max * 1.0001).map(|v| (v, label(v))).collect()
}

/// Log ticks (1, 10, 100 … of `unit`) inside `lo..hi` (natural logs).
pub fn log_ticks(lo: f64, hi: f64, label: impl Fn(f64) -> String) -> Vec<(f64, String)> {
    let mut out = Vec::new();
    let mut v = 10f64.powf((lo / std::f64::consts::LN_10).floor());
    while v.ln() <= hi && out.len() < 6 {
        if v.ln() >= lo {
            out.push((v, label(v)));
        }
        v *= 10.;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(host: &str) -> Raw {
        let path = format!("{}/fixtures/usage/{host}.json", env!("CARGO_MANIFEST_DIR"));
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        Raw::from_fixture(&v).unwrap_or_else(|e| panic!("{host}: {e}"))
    }

    /// `usage-design/spec.md` section 9, all time.
    #[test]
    fn the_fixtures_give_the_specs_numbers() {
        let now = 1_794_654_869_189;
        let cells = |h: &str| {
            let b = Board::build(&fixture(h), (h != "all").then_some(h), Range::All, now);
            b.cells.iter().map(|c| c.number.clone()).collect::<Vec<_>>()
        };
        //            spend          requests    sessions  tokens    cache
        let want = [
            ("all", ["$4,689", "162,810", "1,055", "22.6B", "86.1 %"]),
            ("avante", ["$871.51", "34,083", "3", "9.28B", "87.9 %"]),
            ("bella", ["$1,988", "64,162", "430", "6.22B", "85.2 %"]),
            ("jade", ["$1,307", "64,565", "236", "6.96B", "85.7 %"]),
            ("unknown", ["$522.89", NONE, "386", "132M", NONE]),
        ];
        for (h, w) in want {
            assert_eq!(&cells(h)[..5], &w[..], "{h}");
        }
        // Saved by cache, net.
        let b = Board::build(&fixture("all"), None, Range::All, now);
        assert_eq!(b.cells[4].second, "saved $4,067 net");
        assert_eq!(Board::build(&fixture("bella"), Some("bella"), Range::All, now).cells[4].second, "saved $1,890 net");
        // Workers' share: the role series.
        assert_eq!(b.cells[0].second, "main $4,200 · workers $488.32");
        // The host series add up to `all`.
        let host_total: f64 = b.spend[0].series.iter().map(|l| l.total).sum();
        assert!((host_total - 4_688.67).abs() < 0.05, "{host_total}");
        assert_eq!(b.hosts, vec!["avante", "bella", "jade"], "unknown is never a chip");
        // The largest day.
        let s = &b.spend[0];
        let (day, v) = (0..s.days.len())
            .map(|i| (i, s.series.iter().map(|l| l.values[i].unwrap_or(0.)).sum::<f64>()))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, v)| (s.days[i].clone(), v))
            .unwrap();
        assert_eq!((day.as_str(), money(v)), ("2026-11-11", "$225.02".to_string()));
        assert!((v - 225.02).abs() < 0.01);
        assert_eq!(b.ttft.days.iter().position(|d| d == "2026-11-11").map(|i| fixture("all").latency.daily[i].n), Some(Some(633)));
        // Request-level from 2026-06-12, as the dotted rule, on each stack's own axis.
        for i in 0..3 {
            assert_eq!(b.request_ix[i].map(|ix| b.spend[i].days[ix].as_str()), Some("2026-06-12"), "stack {i}");
        }
        // Formatted once per load: the readout totals and the axes.
        assert_eq!(b.spend[0].series.iter().map(|l| l.total).sum::<f64>().round(), 4_689.);
        assert!(b.spend[0].series.iter().all(|l| l.label == money(l.total)));
        assert!(!b.spend_axes[0].y.is_empty() && b.hit_axes.y.len() == 3);
        // Model view: five and `other`.
        assert_eq!(b.spend[1].series.len(), 6);
        assert_eq!(b.spend[1].series[5].key, "other");
        assert_eq!(b.spend[1].series[0].key, "claude-opus-4-6");
    }

    /// M5: a host chip's HOST chart starts at that host's first day, and
    /// the dotted rule sits on that host's first request-level day there.
    #[test]
    fn a_host_chips_charts_share_its_own_days() {
        let raw = fixture("bella");
        let b = Board::build(&raw, Some("bella"), Range::All, 1_794_654_869_189);
        assert_eq!(b.spend[0].series.iter().map(|l| l.key.as_str()).collect::<Vec<_>>(), ["bella"]);
        assert_eq!(b.spend[0].days.first(), b.spend[1].days.first(), "the HOST axis is bella's, not every host's");
        let day = raw.spend_model.request_day().unwrap();
        for i in 0..3 {
            let ix = b.request_ix[i].unwrap_or_else(|| panic!("stack {i}"));
            assert!(b.spend[i].days[ix].as_str() >= day && b.spend[i].days[ix - 1].as_str() < day, "stack {i}");
        }
        // L10: days under the floor are counted.
        assert_eq!(b.hit_below, raw.cache.daily.iter().filter(|d| d.hit_pct.is_some_and(|v| v < HIT_FLOOR)).count());
    }

    #[test]
    fn nulls_are_dashes_not_zeroes() {
        let jade = Board::build(&fixture("jade"), Some("jade"), Range::All, 1_794_654_869_189);
        // jade has no live numbers: its last hour is unknown.
        assert_eq!(jade.cells[5].number, NONE);
        assert_eq!(jade.cells[1].second, format!("{NONE} today"));
        // A percentile with no data is a gap, not a zero.
        assert!(jade.ttft.series[0].values.iter().any(Option::is_none));
        assert!(jade.ttft.series[0].values.iter().flatten().all(|v| *v > 0.));
        let unknown = Board::build(&fixture("unknown"), Some("unknown"), Range::All, 1_794_654_869_189);
        assert!(unknown.ttft.days.is_empty());
        assert!(unknown.notes.latency.contains("no request-level latency"));
    }

    #[test]
    fn formats() {
        assert_eq!((money(4_688.670_2), money(522.89), money(0.)), ("$4,689".into(), "$522.89".into(), "$0.00".into()));
        assert_eq!(
            (tokens(22_592_939_097), tokens(9_284_193_961), tokens(131_971_011), tokens(812)),
            ("22.6B".into(), "9.28B".into(), "132M".into(), "812".into())
        );
        assert_eq!(
            (secs(1150.), secs(4100.), secs(32_800.), secs(860.)),
            ("1.15 s".into(), "4.10 s".into(), "32.8 s".into(), "860 ms".into())
        );
        assert_eq!(thousands(162_810), "162,810");
        assert_eq!(day_label("2026-11-11"), "Nov 11");
        // 2026-11-14 11:14:29 UTC is 06:14 in New York (EST); 2026-07-01 12:00 UTC is 08:00 EDT.
        assert_eq!(stamp(1_794_654_869_189), "Nov 14 06:14");
        assert_eq!(stamp(1_782_907_200_000), "Jul 1 08:00");
        assert_eq!(ny_day(1_794_654_869_189), "2026-11-14");
        assert_eq!(age(0, 125_000), "2 min");
        assert_eq!(Range::parse("30d"), Some(Range::D30));
        assert_eq!(Range::D7.from(8 * 86_400_000), Some(86_400_000));
        assert_eq!(tracked("avante"), "A V A N T E");
    }

    #[test]
    fn paths_carry_host_and_from_except_the_chip_series() {
        let p = paths(Some("bella"), Some(5));
        assert!(p.iter().any(|(k, u)| *k == "summary" && u == "/api/telemetry/summary?host=bella&from=5"));
        assert!(p.iter().any(|(k, u)| *k == "spend_host" && u == "/api/telemetry/spend?bucket=day&group=host&from=5"));
        assert!(p.iter().any(|(k, u)| *k == "sessions" && u == "/api/telemetry/sessions?sort=cost&limit=25&host=bella&from=5"));
        assert_eq!(paths(None, None)[0].1, "/api/telemetry/summary");
        assert_eq!(source_path("jade", None), "/api/telemetry/summary?host=jade");
        // A host name is a query value, encoded.
        assert_eq!(source_path("a b&c=d", Some(1)), "/api/telemetry/summary?host=a%20b%26c%3Dd&from=1");
        assert!(paths(Some("x/y"), None).iter().any(|(_, u)| u == "/api/telemetry/summary?host=x%2Fy"));
    }

    #[test]
    fn ticks() {
        let days: Vec<String> = ["2026-10-30", "2026-10-31", "2026-11-01", "2026-11-02"].iter().map(|s| s.to_string()).collect();
        // Under 45 days: week starts (Monday 2026-11-02).
        assert_eq!(x_ticks(&days), vec![(3, "Nov 2".to_string())]);
        let ys = y_ticks(225.02, money);
        assert!(ys.len() <= 4 && !ys.is_empty() && ys.last().unwrap().0 <= 225.02, "{ys:?}");
        assert_eq!(log_ticks(1f64.ln(), 50_000f64.ln(), |v| format!("{v}")).len(), 5);
    }
}
