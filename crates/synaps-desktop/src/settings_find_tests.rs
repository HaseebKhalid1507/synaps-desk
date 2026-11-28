//! Settings' find on today's page (settings-overhaul PLAN v3.2 §4, PR 3):
//! the words found drawn lit in their row, a typo finding its row, and the
//! help sentence that holds a hit. Each reads the selector of the element
//! the text was handed to.

use gpui_kit::{AnyWindowHandle, Entity, TestAppContext, VisualTestContext};

use super::{open_at, settle};
use crate::app::AppView;

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
