//! Settings' find on today's page (settings-overhaul PLAN v3.2 §4, PR 3):
//! the words found drawn lit in their row, a typo finding its row, and the
//! help sentence that holds a hit. Each reads the selector of the element
//! the text was handed to. Then what review round 1 found: a query matched
//! once, not each frame; a typo past an unrelated row; a long name's hits
//! within its characters; and the keystroke's bench.

use gpui_kit::{AnyWindowHandle, Entity, ScrollDelta, ScrollEvent, TestAppContext, VisualTestContext, point, px, size};

use super::{open_at, settle, wait};
use crate::app::AppView;
use crate::dash::{Plugin, Provider};
use crate::settings_find as find;

/// Settings over the demo session, found by `find`.
fn settings(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, find: &str) {
    view.update(cx, |v, cx| {
        v.open_view(crate::tabs::Kind::Settings, cx);
        v.settings_find(find.into(), cx);
    });
    settle(cx, w, 8);
}

fn drawn(cx: &mut TestAppContext, w: AnyWindowHandle, sel: &str) -> bool {
    VisualTestContext::from_window(w, cx).debug_bounds(String::leak(sel.to_string())).is_some()
}

fn window(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<AppView>) {
    let (w, view, _events) = open_at(cx, (1440., 900.), crate::theme::Panels::default().text_size);
    (w, view)
}

/// `keep warm` finds Cache keep-warm (`cache_keepwarm`, MORE DAEMON
/// CONFIG's): its row draws hit runs, on the two words in its label and in
/// its key; so does THIS SESSION's Cache keep-warm (`keepwarm`).
#[gpui_kit::test]
fn find_highlights_the_hit(cx: &mut TestAppContext) {
    let (w, view) = window(cx);
    settings(cx, w, &view, "keep warm");
    assert!(drawn(cx, w, "reference cache_keepwarm = —"), "found");
    assert!(drawn(cx, w, "hit-cache_keepwarm"), "a hit run in its row");
    assert!(drawn(cx, w, "hits cache_keepwarm label = keep|warm"), "the label's words lit");
    assert!(drawn(cx, w, "hits cache_keepwarm key = keep|warm"), "the key's");
    assert!(drawn(cx, w, "hit-keepwarm") && drawn(cx, w, "hits keepwarm label = keep|warm"), "the session's row too");
    settings(cx, w, &view, "");
    view.update(cx, |v, cx| {
        v.settings.more_open = true;
        cx.notify();
    });
    settle(cx, w, 8);
    assert!(drawn(cx, w, "reference cache_keepwarm = —") && !drawn(cx, w, "hit-cache_keepwarm"), "no query: nothing lit");
}

/// `thinkng`, one letter short: the Thinking row, its word lit.
#[gpui_kit::test]
fn find_typo_finds_the_row(cx: &mut TestAppContext) {
    let (w, view) = window(cx);
    settings(cx, w, &view, "thinkng");
    assert!(!drawn(cx, w, "settings-nothing thinkng"), "something matches");
    assert!(drawn(cx, w, "row config-thinking"), "the Thinking row");
    assert!(drawn(cx, w, "hits thinking label = Thinking"), "its label's word lit");
}

/// "Remember card choices"' help as today's page words it: its three
/// sentences.
const RECALL: [&str; 3] = [
    "When an agent asks the same card question again in the same folder, the card starts with your last choice highlighted, marked “last time”.",
    "The highlight is only a starting point: nothing is ticked or sent for you, and Enter sends what's ticked (on a one-choice card, the option highlighted).",
    "Only which option you picked is kept, on this machine, never a note or a field's text.",
];

/// Not in PLAN v3.2's UI list: a hit past the help's first sentence shows
/// the sentence that holds it, with `…`; the help opened, all of it.
#[gpui_kit::test]
fn find_shows_the_help_sentence_with_the_hit(cx: &mut TestAppContext) {
    let (w, view) = window(cx);
    let key = crate::recall::SETTING;
    settings(cx, w, &view, "ticked");
    assert!(drawn(cx, w, "row app-recall"), "found by its help");
    assert!(drawn(cx, w, &format!("help-text {key} = … {}", RECALL[1])), "the sentence with the hit");
    assert!(!drawn(cx, w, &format!("help-text {key} = {}", RECALL[0])), "not the first");
    view.update(cx, |v, cx| {
        v.settings.open_help = Some(key);
        cx.notify();
    });
    settle(cx, w, 8);
    assert!(drawn(cx, w, &format!("help-text {key} = {}", RECALL.join(" "))), "opened: all of it");
}

/// The dash's config, this daemon's: `providers` and `plugins` on the
/// page; `every` key exposed and the daemon local, its gate open (as the
/// continuity suite's `local`).
fn with_config(cx: &mut TestAppContext, view: &Entity<AppView>, providers: Vec<Provider>, plugins: Vec<Plugin>, every: bool) {
    view.update(cx, |v, cx| {
        v.model.daemon = Some(crate::model::Daemon { version: "0.13.0".into(), profile: None, pid: 7 });
        v.model.conn = crate::model::ConnState::Connected("0.13.0".into());
        v.dash.serves = Some(Ok(crate::dash::Info { pid: 7, profile: None }));
        let mut snap = crate::dash::ConfigSnapshot { providers, plugins, ..Default::default() };
        if every {
            snap.values = crate::settings::ALL.iter().map(|d| (d.key.to_string(), None)).collect();
            snap.writable = true;
            let target = crate::dash::Target::parse("http://127.0.0.1:7340").expect("a URL");
            v.dash.local = Some(Ok(()));
            v.dash.found = Some(Ok((target, crate::dash::Source::UrlFile("/run/synaps-dash.url".into()))));
            v.dash.answer = crate::views::Answer::Up;
        }
        v.dash.config = Some(snap);
        v.views_changed(cx);
    });
}

fn provider(name: &str) -> Provider {
    Provider { name: name.into(), kind: "openai".into(), source: "config.toml".into(), note: None }
}

/// A query's rows are matched once (review P2-1): the page's scroll, a
/// help opened, a resize and motion draw it again from what was found. A
/// new query, or a change to what the page searches, is matched afresh.
#[gpui_kit::test]
fn find_matches_per_query_not_per_frame(cx: &mut TestAppContext) {
    let (w, view) = window(cx);
    settings(cx, w, &view, "the");
    assert!(!drawn(cx, w, "settings-nothing the"), "rows found");
    let (m, walks) = (find::matched(), find::walks());
    assert!(m > 0, "matched");
    let top = view.read_with(cx, |v, _| v.settings.scroll.offset().y);
    let wheel =
        ScrollEvent { position: point(px(800.), px(500.)), delta: ScrollDelta::Pixels(point(px(0.), px(-400.))), ..Default::default() };
    VisualTestContext::from_window(w, cx).simulate_event(wheel);
    settle(cx, w, 8);
    assert!(view.read_with(cx, |v, _| v.settings.scroll.offset().y) < top, "scrolled");
    view.update(cx, |v, cx| {
        v.settings.open_help = Some("thinking");
        v.views_changed(cx);
    });
    settle(cx, w, 8);
    VisualTestContext::from_window(w, cx).simulate_resize(size(px(1280.), px(860.)));
    settle(cx, w, 8);
    wait(cx, w, &view, 600);
    assert!(find::walks() >= walks + 3, "drawn again: {} walks", find::walks() - walks);
    assert_eq!(find::matched(), m, "no row matched again");
    settings(cx, w, &view, "thinkng");
    assert!(find::matched() > m && drawn(cx, w, "row config-thinking"), "a new query, matched");
    settings(cx, w, &view, "zebra");
    assert!(drawn(cx, w, "settings-nothing zebra"), "nothing yet");
    let m = find::matched();
    with_config(cx, &view, vec![provider("zebra-gateway")], Vec::new(), false);
    settle(cx, w, 8);
    assert!(find::matched() > m, "what the page searches changed: matched again");
    assert!(drawn(cx, w, "fact zebra-gateway = openai · from config.toml"), "and found");
    assert!(drawn(cx, w, "hits zebra-gateway label = zebra"), "lit");
}

/// `content window` finds Context window, `content` one edit from
/// `context`, though Compaction exclude's help has `Content` as typed
/// (review P2-2).
#[gpui_kit::test]
fn find_reads_a_typo_past_an_unrelated_row(cx: &mut TestAppContext) {
    let (w, view) = window(cx);
    settings(cx, w, &view, "content window");
    assert!(!drawn(cx, w, "settings-nothing content window"), "something matches");
    assert!(drawn(cx, w, "row config-context_window"), "Context window");
    assert!(drawn(cx, w, "hits context_window label = Context|window"), "its words lit");
}

/// A plugin named past 65,535 bytes, a character across the cut: its hits
/// end at characters (review P2-4). A typo the cut ends inside finds
/// nothing; the word as typed is lit.
#[gpui_kit::test]
fn find_lights_a_long_name_within_its_characters(cx: &mut TestAppContext) {
    let (w, view) = window(cx);
    let name = format!("{}café", " ".repeat(65_531));
    let plugin = Plugin { name: name.clone(), scope: "user".into(), enabled: true, ..Default::default() };
    with_config(cx, &view, Vec::new(), vec![plugin], false);
    settings(cx, w, &view, "caff");
    assert!(!drawn(cx, w, &format!("fact {name} = enabled · user")), "a typo the cut ends inside: not found");
    settings(cx, w, &view, "caf");
    assert!(drawn(cx, w, &format!("hits {name} label = caf")), "the word as typed, lit");
}

/// The keystroke's release bench (PLAN §3.6, review round 1). Each query
/// is set as `settings_find` sets it, then the page drawn by itself as its
/// pane draws it, its first draw for the query (`page`: the match, its
/// sort and the page's elements), the page drawn again with the query the
/// same (`page again`), then the window told and its next frames (`frame`:
/// the window drawn, laid out and painted on the test platform). And the
/// page with no query (what any draw of the whole page costs). The largest
/// legitimate inventory: 64 providers and 128 plugins (names at 64 bytes)
/// from a local daemon whose dash exposes every key. The matcher bench's
/// 1,000 queries, each new, then the worst cases, each new too. On bella:
/// `cargo test --release -p synaps-desktop -- --ignored settings_find_keystroke_bench --nocapture`.
#[gpui_kit::test]
#[ignore]
fn settings_find_keystroke_bench(cx: &mut TestAppContext) {
    let (w, view) = window(cx);
    let name64 = |s: String| format!("{s}{}", "-and-a-name-as-long-as-a-name-may-be").chars().take(64).collect::<String>();
    let providers = (0..64).map(|i| provider(&name64(format!("provider-{i:02}-openrouter-compatible-gateway")))).collect();
    let help = "Adds tools for the agent: reads the repository, runs its checks, and reports what changed, with a note per file touched.";
    let plugins = (0..128)
        .map(|i| Plugin {
            name: name64(format!("plugin-{i:03}-tools-for-the-repository")),
            scope: "user".into(),
            description: help.into(),
            version: Some("1.4.2".into()),
            enabled: true,
        })
        .collect();
    with_config(cx, &view, providers, plugins, true);
    settings(cx, w, &view, "");
    let words: Vec<String> = crate::settings::ALL.iter().flat_map(|d| d.label.split(' ')).map(str::to_lowercase).collect();
    let mut queries: Vec<String> = Vec::with_capacity(1100);
    'typed: for w in words.iter().cycle() {
        for end in 1..=w.len() {
            if queries.len() == 700 {
                break 'typed;
            }
            if w.is_char_boundary(end) {
                queries.push(w[..end].to_string());
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
    for a in ["colour", "font", "zoom", "dark", "ctx", "effort", "reasoning", "retry", "notify", "sidebar"].iter().cycle().take(100) {
        queries.push(a.to_string());
    }
    queries.extend((0..50).map(|i| format!("zzq{i}")));
    // The worst cases, each new: 16 one-letter words; 15 and a typo that
    // misses (each row read to its 16th word, then typo-read); 16 words
    // every plugin has as typed (the most hits); 16 long typos of the
    // plugins' words (no row has one as typed: every row typo-read).
    let worst = [
        "e a t o i n s r l c u d p m h g",
        "e a t o i n s r l c u d p m h zzzzq",
        "plugin tools for the repository and name as long may be adds agent reads runs checks reports",
        "plugn toolz repositry agnet reds runz chekcs reprots chnaged tuoched notte fiel enabeld usre toosl adds",
    ];
    let first_worst = queries.len();
    for i in 0..40 {
        queries.push(worst[i % worst.len()].to_string());
        queries.push(format!("zzq-between-{i}"));
    }
    // The page drawn by itself, as its pane draws it.
    let draw = |cx: &mut TestAppContext| {
        let at = std::time::Instant::now();
        cx.update(|cx| {
            let cx: &gpui_kit::App = cx;
            let p = crate::theme::palette(cx);
            let a = view.read(cx);
            let vc = crate::views::ViewCx::of(view.downgrade(), cx);
            std::hint::black_box(a.render_settings(a.view_width, &p, &vc));
        });
        at.elapsed()
    };
    // The page with no query: what any draw of it costs.
    let mut bare: Vec<_> = (0..100).map(|_| draw(cx)).collect();
    let n = queries.len();
    let (mut page, mut again, mut frame) = (Vec::with_capacity(n), Vec::with_capacity(n), Vec::with_capacity(n));
    let mut worst = std::time::Duration::ZERO;
    for (i, t) in queries.iter().enumerate() {
        // The query as `settings_find` sets it, the window not yet told: a
        // test app draws a window told at the end of the update (GPUI's
        // `flush_effects`), and this draw is the page's first.
        view.update(cx, |v, _| {
            v.settings.search.get_mut().set(find::Query::parse(t));
        });
        let drawn = draw(cx);
        page.push(drawn);
        if i >= first_worst && i % 2 == (first_worst % 2) {
            worst = worst.max(drawn);
        }
        again.push(draw(cx));
        // The window told, as `settings_find` tells it: its frame.
        let at = std::time::Instant::now();
        view.update(cx, |v, cx| v.views_changed(cx));
        settle(cx, w, 2);
        frame.push(at.elapsed());
    }
    let us = |d: std::time::Duration| d.as_secs_f64() * 1e6;
    let line = |what: &str, v: &mut Vec<std::time::Duration>| {
        v.sort();
        let n = v.len();
        println!(
            "settings_find_keystroke_bench {what}: {n}: p50 {:.1} µs, p99 {:.1} µs, max {:.1} µs",
            us(v[n / 2]),
            us(v[n * 99 / 100]),
            us(v[n - 1])
        );
    };
    line("page, a new query (its first draw: match, sort, build), queries", &mut page);
    line("page again (the same query drawn again), queries", &mut again);
    line("frame (the window told: drawn, laid out and painted), queries", &mut frame);
    line("no query (the whole page drawn), draws", &mut bare);
    println!("settings_find_keystroke_bench worst cases: slowest page, a new query {:.1} µs", us(worst));
}
