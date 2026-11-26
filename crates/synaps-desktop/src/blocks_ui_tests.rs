//! D5's desk side after Shady's review of #101 (`review-pr-desk101-shady.md`):
//! the edit preview's text drawn as D1 draws agent text (P2-9), a file's
//! header (P2-10), and the `op skipped` chip. Each fails on `653c30c`.

use gpui_kit::{
    AnyWindowHandle, Entity, ListOffset, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, TestAppContext,
    VisualTestContext, px,
};
use serde_json::{Value, json};
use synaps_client::synthetic::{block_input, block_receipt};

use super::answer_tests::{has, heard, llm};
use super::{open_at, settle, wait};
use crate::app::AppView;
use crate::turns::Kind;

type W = (AnyWindowHandle, Entity<AppView>);

/// Session `id` opened in `place`, attached by the demo's transport.
fn open_session(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, id: &str, place: crate::tabs::Place) {
    let client = view.update(cx, |v, cx| {
        let c = v.client.take();
        v.open_tab(crate::app::Open::Attach { session_id: id.into(), fallback: None, open: false }, place);
        cx.notify();
        c
    });
    cx.run_until_parked();
    settle(cx, w, 8);
    view.update(cx, |v, _| v.client = client);
    board(cx, w, view);
    wait(cx, w, view, 600);
}

/// The blocks demo session in a window `w` px wide at text step `text`,
/// its board drawn.
fn blocks_at(cx: &mut TestAppContext, w: f32, text: u8) -> W {
    let (win, view, _events) = open_at(cx, (w, 1000.), text);
    open_session(cx, win, &view, crate::demo::BLOCKS, crate::tabs::Place::Current);
    (win, view)
}

fn blocks(cx: &mut TestAppContext, w: f32) -> W {
    blocks_at(cx, w, crate::theme::Panels::default().text_size)
}

/// The board's job runs and lands, and what it drew settles.
fn board(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    for _ in 0..4 {
        view.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        settle(cx, w, 8);
    }
}

/// Block `name`'s row in the pane acted on: its id and display index.
fn row(cx: &mut TestAppContext, view: &Entity<AppView>, name: &str) -> Option<(u64, usize)> {
    view.read_with(cx, |v, _| {
        let items = v.model.lives[v.panes.cur.live].shown().items();
        v.panes.cur.disp.iter().enumerate().find_map(|(ix, d)| {
            let Kind::Block { at } = d.kind else { return None };
            let s = items.get(d.item)?.blocks.as_ref()?.blocks.get(at as usize)?;
            (s.block.id.to_string() == name).then_some((d.id, ix))
        })
    })
}

fn id(cx: &mut TestAppContext, view: &Entity<AppView>, name: &str) -> u64 {
    row(cx, view, name).unwrap_or_else(|| panic!("block {name} drawn")).0
}

fn show_block(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, name: &str) -> u64 {
    let (id, ix) = row(cx, view, name).unwrap_or_else(|| panic!("block {name} drawn"));
    view.update(cx, |v, cx| {
        v.panes.cur.list.scroll_to(ListOffset { item_ix: ix, offset_in_item: px(0.) });
        cx.notify();
    });
    cx.run_until_parked();
    settle(cx, w, 8);
    id
}

fn bounds(cx: &mut TestAppContext, w: AnyWindowHandle, sel: String) -> Option<gpui_kit::Bounds<gpui_kit::Pixels>> {
    VisualTestContext::from_window(w, cx).debug_bounds(String::leak(sel))
}

fn drawn(cx: &mut TestAppContext, w: AnyWindowHandle, sel: String) -> gpui_kit::Bounds<gpui_kit::Pixels> {
    bounds(cx, w, sel.clone()).unwrap_or_else(|| panic!("{sel} drawn"))
}

/// A live round of block calls (`(tool, ops, bound)`); the board lands.
fn round(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, calls: &[(&str, Value, bool)]) {
    let mut evs = vec![llm(json!({"kind": "llm", "llm": "response_start"}))];
    for (tool, ops, _) in calls {
        evs.push(llm(json!({"kind": "llm", "llm": "tool_use_start", "tool_name": "cards_block", "tool_id": tool})));
        evs.push(llm(
            json!({"kind": "llm", "llm": "tool_use", "tool_name": "cards_block", "tool_id": tool, "input": block_input(ops.clone())}),
        ));
    }
    for (tool, ops, bound) in calls {
        if *bound {
            evs.push(llm(json!({"kind": "llm", "llm": "tool_result", "tool_id": tool, "result": block_receipt(ops)})));
        }
    }
    heard(cx, w, view, evs);
    board(cx, w, view);
}

fn turn(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    heard(cx, w, view, vec![json!({"ev": "turn_started", "turn_baseline": 13, "trigger": "user", "user_text": "go on"})]);
}

fn mouse(cx: &mut TestAppContext, w: AnyWindowHandle, at: gpui_kit::Point<gpui_kit::Pixels>, ev: &str) {
    let mut vcx = VisualTestContext::from_window(w, cx);
    match ev {
        "move" => vcx.simulate_event(MouseMoveEvent { position: at, pressed_button: None, modifiers: Modifiers::default() }),
        "drag" => {
            vcx.simulate_event(MouseMoveEvent { position: at, pressed_button: Some(MouseButton::Left), modifiers: Modifiers::default() })
        }
        "down" => vcx.simulate_event(MouseDownEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        }),
        _ => vcx.simulate_event(MouseUpEvent { button: MouseButton::Left, position: at, modifiers: Modifiers::default(), click_count: 1 }),
    }
    vcx.run_until_parked();
    settle(cx, w, 8);
}

/// A live turn putting `ops` and twelve notes under them, the first block
/// at the view's top (the list not following its tail).
fn put_mid(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, ops: Value) {
    turn(cx, w, view);
    let mut all = ops.as_array().cloned().unwrap_or_default();
    let first = all[0]["id"].as_str().unwrap_or_default().to_string();
    all.extend((0..12).map(|k| json!({"op":"put","id":format!("under{k}"),"kind":"note","body":{"text":"under"}})));
    round(cx, w, view, &[("toolu_p", json!(all), true)]);
    show_block(cx, w, view, &first);
    wait(cx, w, view, 1000);
}

/// P2-9: an edit preview's lines are drawn as D1 draws agent text: a
/// successful edit, write or multi-edit whose text holds bidi controls,
/// isolates, control characters or invisibles reaches the row without
/// them (the requested edit, and the ledger, keep their bytes).
#[test]
fn edit_preview_lines_are_sanitized() {
    let evil = "let s = \"\u{202E}lave\u{202C}\"; \u{2066}x\u{2069} \u{200B}zw\u{0007}bell\u{001B}[31m";
    let tool = |id: &str, name: &str, input: Value| json!({"role": "assistant", "content": [{"type": "tool_use", "id": id, "name": name, "input": input}]});
    let ok = |id: &str| json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": id, "content": "ok"}]});
    let msgs = vec![
        json!({"role": "user", "content": "go"}),
        tool("t1", "edit", json!({"path": "/w/a.rs", "old_string": "x", "new_string": evil})),
        ok("t1"),
        tool("t2", "write", json!({"path": "/w/a.rs", "content": format!("{evil}\nline two")})),
        ok("t2"),
        tool("t3", "multi_edit", json!({"path": "/w/a.rs", "edits": [{"old_string": "y", "new_string": evil}]})),
        ok("t3"),
    ];
    let mut t = synaps_client::Transcript::new();
    t.apply_history(&msgs);
    let mut ledger = crate::files::Ledger::default();
    ledger.reset(&msgs, None, false);
    let (groups, _) = crate::blocks::edit_preview(t.items(), &ledger, "/w/a.rs");
    assert!(groups.len() >= 2, "the edits drawn: {}", groups.len());
    let bad = |c: char| matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200B}') || (c.is_control() && c != '\t');
    for g in &groups {
        for l in &g.lines {
            assert!(!l.text.chars().any(bad), "a raw character reached the row: {:?}", l.text);
        }
    }
    assert!(groups.iter().flat_map(|g| &g.lines).any(|l| l.text.contains("lave")), "the text itself is drawn");
}

/// P2-10: a file's header holds its path, `:line` and note; a title comes
/// before them and never hides the file's name and line. At 320 px and
/// wide.
#[gpui_kit::test]
fn a_file_header_shows_its_path_line_and_note(cx: &mut TestAppContext) {
    for width in [320. + 2. * crate::app::STRIP, 1500.] {
        let (w, view) = blocks(cx, width);
        let wide = width > 600.;
        let put = json!([
            {"op":"put","id":"f1","kind":"file","body":{"path":"crates/relay/src/token_store.rs","line":118,"note":"TokenStore::rotate","state":"editing"}},
            {"op":"put","id":"f2","kind":"file","title":"The token store, where rotate() takes its lock per session","body":{"path":"crates/relay/src/token_store.rs","line":118,"note":"TokenStore::rotate","state":"editing"}},
            {"op":"put","id":"f3","kind":"file","body":{"path":"crates/relay/src/session.rs","state":"reading"}},
            {"op":"put","id":"f4","kind":"file","title":"Session","body":{"path":"crates/relay/src/session.rs","state":"done"}}
        ]);
        put_mid(cx, w, &view, put);
        for (name, shown, has_note, titled) in [
            ("f1", "token_store.rs:118", true, false),
            ("f2", "token_store.rs:118", true, true),
            ("f3", "session.rs", false, false),
            ("f4", "session.rs", false, true),
        ] {
            let f = id(cx, &view, name);
            let block = drawn(cx, w, format!("block {f}"));
            let path = drawn(cx, w, format!("block-path {f} {shown}"));
            assert!(path.left() >= block.left() && path.right() <= block.right() + px(0.5), "{width} {name}: the path inside its block");
            // The name and line whole: as wide as their characters.
            let m = view.read_with(cx, |v, _| v.panes.cur.metrics_last.get().expect("drawn"));
            let min = shown.chars().count() as f32 * m.mono * 0.5;
            assert!(
                f32::from(path.size.width) >= min,
                "{width} {name}: `{shown}` whole ({} px, {min} at least)",
                f32::from(path.size.width)
            );
            assert!((f32::from(block.size.height) - crate::blocks::Geo::of(&m).head).abs() < 0.5, "{width} {name}: one line");
            if titled {
                let title = drawn(cx, w, format!("block-title {f}"));
                assert!(title.right() <= path.left() + px(0.5), "{width} {name}: the title before the path");
            }
            if has_note && wide {
                let n = drawn(cx, w, format!("block-file-note {f} TokenStore::rotate"));
                assert!(f32::from(n.size.width) > 40. && n.right() <= block.right(), "{width} {name}: the note");
            }
        }
    }
}

/// The plan's chip says `op skipped` (an op, not the plan), and its
/// tooltip says why it is there.
#[gpui_kit::test]
fn the_skipped_chip_is_an_ops_and_says_why(cx: &mut TestAppContext) {
    let (w, view) = blocks(cx, 1500.);
    let plan = show_block(cx, w, &view, "plan");
    assert!(has(cx, w, format!("block-skipped-says {plan} op skipped")), "`op skipped`");
    let chip = drawn(cx, w, format!("block-skipped {plan}")).center();
    mouse(cx, w, chip, "move");
    wait(cx, w, &view, 800);
    mouse(cx, w, chip, "move");
    assert!(has(cx, w, format!("block-skipped-tip {plan}")), "its reason in a tooltip");
}
