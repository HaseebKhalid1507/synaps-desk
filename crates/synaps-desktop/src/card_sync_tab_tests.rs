//! A tab left mid-turn and the card sync (Shady #102 P2-1): the tab cache
//! keeps none of what the sync keeps to read what changed (the attach
//! that shows the tab again reads its cards anew), and its estimate
//! ([`crate::app::TabCache::bytes`]) counts whatever of it a transcript
//! does keep.

use gpui_kit::TestAppContext;
use serde_json::{Value, json};

use super::answer_tests::{heard, llm, open};

/// A live turn, still streaming: `refused` card calls the extension
/// refuses (1 MiB of string in each input), and `other` calls of another
/// tool.
fn turn(refused: usize, other: usize) -> Vec<Value> {
    let mut out = vec![json!({"ev": "turn_started", "turn_baseline": 40, "trigger": "user", "user_text": "go"})];
    let mut call = |name: &str, id: String, input: Value, result: &str| {
        out.push(llm(json!({"kind": "llm", "llm": "tool_use_start", "tool_name": name, "tool_id": id})));
        out.push(llm(json!({"kind": "llm", "llm": "tool_use", "tool_name": name, "tool_id": id, "input": input})));
        out.push(llm(json!({"kind": "llm", "llm": "tool_result", "tool_id": id, "result": result})));
    };
    for k in 0..refused {
        call("cards_ui", format!("r{k}"), json!({"bogus": "x".repeat(1 << 20)}), "cards_ui: refused: no question");
    }
    for k in 0..other {
        call("bash", format!("b{k}"), json!({"command": "ls"}), "a b");
    }
    out
}

/// The card session's tab, mid-turn (four refused 1 MiB inputs, 300
/// calls), left for another session: cached with its cards and none of
/// its card sync. Had it kept that, its estimate would count it: a budget
/// the rest fits and that does not is past.
#[gpui_kit::test]
fn a_tab_left_mid_turn_is_cached_without_its_card_sync(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx, crate::demo::ASKS);
    heard(cx, w, &view, turn(4, 300));
    let (session, kept) = view.read_with(cx, |v, _| {
        let session = v.model.tabs.current().and_then(|t| t.session.clone()).expect("its session");
        (session, v.model.lives[v.panes.cur.live].transcript.clone())
    });
    let sync = kept.card_sync_bytes();
    let client = view.update(cx, |v, cx| {
        let c = v.client.take();
        let other = crate::app::Open::Attach { session_id: crate::demo::IDLE_CPU.into(), fallback: None, open: false };
        v.open_tab(other, crate::tabs::Place::New);
        cx.notify();
        c
    });
    cx.run_until_parked();
    view.update(cx, |v, _| v.client = client);
    view.update(cx, |v, _| {
        let mut e = v.tab_cache.take(&session).expect("the tab left is cached");
        assert_eq!(e.transcript.card_sync_bytes(), 0, "cached: its card sync let go");
        assert_eq!(e.transcript.cards().cards(), kept.cards().cards(), "cached: its cards kept");
        // The tab as it was left, with its card sync and without.
        let mut bare = kept.clone();
        bare.shed_card_sync();
        e.transcript = bare;
        let shed = e.bytes();
        e.transcript = kept;
        let whole = e.bytes();
        assert!(whole >= shed + sync, "{whole} bytes estimated with the card sync, {shed} without, {sync} of it");
        let mut cache = crate::tabs::Lru::new(crate::app::TAB_CACHE, shed + sync / 2);
        cache.put(session.clone(), e, whole, |_| true);
        assert_eq!(cache.len(), 0, "past a budget the rest fits by its card sync: not kept");
    });
    // What it kept to count: digests and places, not the inputs again.
    assert!(sync > 0 && sync < 256 << 10, "the card sync kept {sync} bytes for 4 MiB of inputs and 300 calls");
}
