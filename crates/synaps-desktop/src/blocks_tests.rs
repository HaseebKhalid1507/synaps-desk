//! Blocks in the window (synaps-cards `docs/blocks.md` v1.3, D5's tests in
//! §8.2): a render per kind and state at 320 px and wide (§6.2's heights,
//! §6.4's order of giving way); binding and the re-run (a pending call,
//! then bound); live and the history drawing the same; anchoring above the
//! view and the pill; §6.2's lease tests; Reduced and Off. The board's
//! work runs off the frame ([`crate::board`]): each step here lets it land.

use gpui_kit::{
    AnyWindowHandle, Entity, ListOffset, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, TestAppContext,
    VisualTestContext, px,
};
use serde_json::{Value, json};
use synaps_client::synthetic::{block_input, block_receipt};

use super::answer_tests::{has, heard, land, llm};
use super::{motion_level, open_at, settle, wait};
use crate::app::AppView;
use crate::turns::Kind;

type W = (AnyWindowHandle, Entity<AppView>);

/// The blocks demo session in a window `w` px wide (both panels at their
/// strips under 600), its board drawn.
fn blocks(cx: &mut TestAppContext, w: f32) -> W {
    let (win, view, _events) = open_at(cx, (w, 1000.), crate::theme::Panels::default().text_size);
    let client = view.update(cx, |v, cx| {
        let c = v.client.take();
        v.open_tab(
            crate::app::Open::Attach { session_id: crate::demo::BLOCKS.into(), fallback: None, open: false },
            crate::tabs::Place::Current,
        );
        cx.notify();
        c
    });
    cx.run_until_parked();
    settle(cx, win, 8);
    view.update(cx, |v, _| v.client = client);
    board(cx, win, &view);
    wait(cx, win, &view, 600);
    (win, view)
}

/// The board's job runs and lands, and what it drew settles.
fn board(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    for _ in 0..4 {
        view.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        settle(cx, w, 8);
    }
}

/// Block `name`'s row: its id and display index.
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

/// Display row `ix` at the view's top.
fn show(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, ix: usize) {
    view.update(cx, |v, cx| {
        v.panes.cur.list.scroll_to(ListOffset { item_ix: ix, offset_in_item: px(0.) });
        cx.notify();
    });
    cx.run_until_parked();
    settle(cx, w, 8);
}

/// Block `name` at the view's top, drawn.
fn show_block(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, name: &str) -> u64 {
    let (id, ix) = row(cx, view, name).unwrap_or_else(|| panic!("block {name} drawn"));
    show(cx, w, view, ix);
    id
}

fn bounds(cx: &mut TestAppContext, w: AnyWindowHandle, sel: String) -> Option<gpui_kit::Bounds<gpui_kit::Pixels>> {
    VisualTestContext::from_window(w, cx).debug_bounds(String::leak(sel))
}

fn h(cx: &mut TestAppContext, w: AnyWindowHandle, sel: String) -> f32 {
    f32::from(bounds(cx, w, sel.clone()).unwrap_or_else(|| panic!("{sel} drawn")).size.height)
}

fn geo(cx: &mut TestAppContext, view: &Entity<AppView>) -> crate::blocks::Geo {
    view.read_with(cx, |v, _| crate::blocks::Geo::of(&v.panes.cur.metrics_last.get().expect("drawn")))
}

/// A live round on the session: `calls` block calls of `ops` each, their
/// results given or not; the board lands.
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

fn result(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, tool: &str, ops: &Value) {
    heard(cx, w, view, vec![llm(json!({"kind": "llm", "llm": "tool_result", "tool_id": tool, "result": block_receipt(ops)}))]);
    board(cx, w, view);
}

/// A turn starts on the session.
fn turn(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    heard(cx, w, view, vec![json!({"ev": "turn_started", "turn_baseline": 13, "trigger": "user", "user_text": "go on"})]);
}

fn mouse(cx: &mut TestAppContext, w: AnyWindowHandle, at: gpui_kit::Point<gpui_kit::Pixels>, ev: &str) {
    let mut vcx = VisualTestContext::from_window(w, cx);
    match ev {
        "move" => vcx.simulate_event(MouseMoveEvent { position: at, pressed_button: None, modifiers: Modifiers::default() }),
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

/// Block row `id`'s header's centre.
fn head_at(cx: &mut TestAppContext, w: AnyWindowHandle, id: u64) -> gpui_kit::Point<gpui_kit::Pixels> {
    bounds(cx, w, format!("block-head {id}")).unwrap_or_else(|| panic!("block {id}'s header drawn")).center()
}

fn folded(cx: &mut TestAppContext, view: &Entity<AppView>, id: u64) -> bool {
    view.read_with(cx, |v, _| {
        let (_, s) = v.block_shown(id).expect("drawn");
        v.panes.cur.block_folds.folded(id, &s)
    })
}

/// A live turn putting `ops` and twelve notes under them (16 ops a call), the first block at
/// the view's top (the list not following its tail: what is under a block
/// that grows moves down).
fn put_mid(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, ops: Value) {
    turn(cx, w, view);
    let mut all = ops.as_array().cloned().unwrap_or_default();
    let first = all[0]["id"].as_str().unwrap_or_default().to_string();
    all.extend((0..12).map(|k| json!({"op":"put","id":format!("under{k}"),"kind":"note","body":{"text":"under"}})));
    round(cx, w, view, &[("toolu_p", json!(all), true)]);
    show_block(cx, w, view, &first);
    wait(cx, w, view, 1000);
}

fn note(cx: &mut TestAppContext, view: &Entity<AppView>) -> Option<&'static str> {
    view.read_with(cx, |v, _| v.panes.cur.lease.note.map(|n| n.1))
}

/// Every kind and state the demo holds draws in a 320 px pane and a wide
/// one, at §6.2's heights: a header of 28 px; a plan's steps 28 px each;
/// a status two 20 px lines; a diff's well 20 px a line, 12 at most; a
/// note 6 lines at most, `more` past them; a metric 44 px and 16 with a
/// series; a file, and a block folded, the header alone. Its parts sit in
/// a row inside it, nothing over anything; at 320 px the step notes give
/// way first, then `rN`, never the glyph or the title. A bound block call
/// draws no row; a refused one is a plain row.
#[gpui_kit::test]
fn every_kind_and_state_draws_narrow_and_wide(cx: &mut TestAppContext) {
    for width in [320. + 2. * crate::app::STRIP, 1500.] {
        let (w, view) = blocks(cx, width);
        let g = geo(cx, &view);
        let narrow = width < 600.;
        // Unfold the group (groups are made folded) and the edit preview.
        for name in ["why", "edits"] {
            let id = show_block(cx, w, &view, name);
            assert!(folded(cx, &view, id), "{name} made folded");
            let at = head_at(cx, w, id);
            mouse(cx, w, at, "move");
            mouse(cx, w, at, "down");
            mouse(cx, w, at, "up");
            assert!(!folded(cx, &view, id), "{name} unfolded by a click");
        }
        let steps = |cx: &mut TestAppContext, view: &Entity<AppView>, name: &str| {
            view.read_with(cx, |v, _| {
                let (_, s) = v.block_shown(row_of(v, name)).expect("drawn");
                match &s.block.body {
                    synaps_client::blocks::Body::Plan(p) => p.steps.len(),
                    _ => 0,
                }
            })
        };
        let plan = show_block(cx, w, &view, "plan");
        assert_eq!(h(cx, w, format!("block-head {plan}")), g.head);
        let n = steps(cx, &view, "plan");
        assert!((h(cx, w, format!("block-steps {plan}")) - (n as f32 * g.step + pad(cx, &view))).abs() < 0.5, "a plan: steps × 28");
        assert!(has(cx, w, format!("block-skipped {plan}")), "the skipped chip");
        assert!(has(cx, w, format!("block-step {plan} test")));
        let now = show_block(cx, w, &view, "now");
        assert!((h(cx, w, format!("block-status {now}")) - (2. * g.line + pad(cx, &view))).abs() < 0.5, "a status: two lines");
        let retry = show_block(cx, w, &view, "retry");
        let lines = 2 + 5 + 1; // two hunks' headers and their lines
        assert!(
            (h(cx, w, format!("block-well {retry}")) - lines.min(crate::blocks::DIFF_LINES) as f32 * g.line).abs() < 0.5,
            "a diff's well"
        );
        let file = show_block(cx, w, &view, "file");
        assert!((h(cx, w, format!("block {file}")) - g.head).abs() < 0.5, "a file: its header");
        let edits = show_block(cx, w, &view, "edits");
        assert!(has(cx, w, format!("block-diff {edits}")), "the edit preview, unfolded");
        let card = show_block(cx, w, &view, &format!("card:{}", synaps_client::synthetic::BLOCKS_CARD));
        assert!(has(cx, w, format!("block-note {card}")), "a converted card's note");
        let why = show_block(cx, w, &view, "why");
        let rot = view.read_with(cx, |v, _| {
            let (_, s) = v.block_shown(why).expect("drawn");
            crate::blocks::row_id(&s.members[1].block.id)
        });
        assert!(has(cx, w, format!("block-member {why} {rot}")), "the group's members");
        assert!((h(cx, w, format!("block-metric {rot}")) - (g.metric + g.series + pad(cx, &view))).abs() < 0.5, "a metric with a series");
        // Each header's parts in a row, inside its block, nothing over anything.
        for name in ["plan", "now", "retry", "file", "edits"] {
            let id = show_block(cx, w, &view, name);
            let block = bounds(cx, w, format!("block {id}")).expect("drawn");
            let head = bounds(cx, w, format!("block-head {id}")).expect("drawn");
            assert!(head.left() >= block.left() && head.right() <= block.right() + px(0.5), "{name}'s header inside it");
            let title = bounds(cx, w, format!("block-title {id}")).expect("its title");
            assert!(title.size.width > px(20.), "{name}'s title keeps room");
            let rev = view.read_with(cx, |v, _| {
                let (_, s) = v.block_shown(id).expect("drawn");
                format!("block-rev {id} {}", rev_text(&s))
            });
            assert_eq!(has(cx, w, rev.clone()), !narrow, "{rev}: rN gives way at 320 px only");
            if let Some(r) = bounds(cx, w, rev) {
                assert!(r.left() >= title.right(), "{name}: rN after the title");
            }
        }
        // The narrow pane gives way: the plan's step notes first.
        let plan = show_block(cx, w, &view, "plan");
        let _ = plan;
        assert_eq!(view.read_with(cx, |v, _| crate::blocks::Fit::of(v.panes.cur.layout.column).notes), !narrow);
        // A bound block call draws no row; the refused one is a plain row.
        let rows: Vec<(u64, Option<String>)> = view.read_with(cx, |v, _| {
            let items = v.model.lives[v.panes.cur.live].shown().items();
            v.panes
                .cur
                .disp
                .iter()
                .filter(|d| d.kind == Kind::Work)
                .map(|d| (d.id, items[d.item].tool.as_ref().map(|t| t.tool_id.clone())))
                .collect()
        });
        let tools: Vec<&str> = rows.iter().filter_map(|r| r.1.as_deref()).collect();
        assert!(!tools.contains(&"toolu_b01") && !tools.contains(&"toolu_b06"), "bound calls draw no row: {tools:?}");
        assert!(tools.contains(&"toolu_b08"), "the refused call is a plain row: {tools:?}");
        // The run the agent titled folds under its title.
        let titled = view.read_with(cx, |v, _| v.panes.cur.disp.iter().find(|d| matches!(d.kind, Kind::Summary { .. })).map(|d| d.id));
        let titled = titled.expect("a folded run");
        let at = view.read_with(cx, |v, _| v.panes.cur.disp.iter().position(|d| d.id == titled && matches!(d.kind, Kind::Summary { .. })));
        show(cx, w, &view, at.expect("its row"));
        assert!(has(cx, w, format!("run-title {titled}")), "the agent's run title");
    }
}

fn row_of(v: &AppView, name: &str) -> u64 {
    let items = v.model.lives[v.panes.cur.live].shown().items();
    v.panes
        .cur
        .disp
        .iter()
        .find_map(|d| {
            let Kind::Block { at } = d.kind else { return None };
            let s = items.get(d.item)?.blocks.as_ref()?.blocks.get(at as usize)?;
            (s.block.id.to_string() == name).then_some(d.id)
        })
        .expect("drawn")
}

fn rev_text(s: &synaps_client::blocks::Shown) -> String {
    let b = &s.block;
    let kind = match &b.body {
        synaps_client::blocks::Body::Diff(d) if d.from.is_some() => "EDIT PREVIEW".to_string(),
        synaps_client::blocks::Body::Group(g) => format!("group · {}", g.members.len()),
        body => body.kind().as_str().to_string(),
    };
    let rev = if b.pending {
        "pending".to_string()
    } else if b.replaced.is_some() {
        format!("r{} · replaced", b.rev)
    } else {
        format!("r{}", b.rev)
    };
    format!("{kind} · {rev}")
}

/// A body's bottom padding at the text step.
fn pad(cx: &mut TestAppContext, view: &Entity<AppView>) -> f32 {
    view.read_with(cx, |v, _| (6. * v.panes.cur.metrics_last.get().expect("drawn").r).round())
}

/// Binding and the re-run (§1.6, §1.7): a block call whose result hasn't
/// come draws its block pending (`pending` where `rN` is) and no tool row;
/// its result binds it (`r1`). A refused one is a plain row. The replaced
/// status reads `r1 · replaced`; a skipped op leaves its chip.
#[gpui_kit::test]
fn a_call_is_pending_until_its_result_binds_it(cx: &mut TestAppContext) {
    let (w, view) = blocks(cx, 1500.);
    let now = show_block(cx, w, &view, "now");
    assert!(has(cx, w, format!("block-rev {now} status · r1 · replaced")));
    turn(cx, w, &view);
    let put = json!([{"op":"put","id":"live","kind":"status","body":{"text":"Bisecting","state":"working"}}]);
    round(cx, w, &view, &[("toolu_l1", put.clone(), false)]);
    let live = id(cx, &view, "live");
    view.update(cx, |v, cx| {
        v.panes.cur.list.scroll_to_end();
        cx.notify();
    });
    settle(cx, w, 8);
    assert!(has(cx, w, format!("block-rev {live} status · pending")), "pending: no rN");
    let tool_rows = |cx: &mut TestAppContext| {
        view.read_with(cx, |v, _| {
            let items = v.model.lives[v.panes.cur.live].shown().items();
            v.panes
                .cur
                .disp
                .iter()
                .filter(|d| d.kind == Kind::Work && items[d.item].tool.as_ref().is_some_and(|t| t.tool_id == "toolu_l1"))
                .count()
        })
    };
    assert_eq!(tool_rows(cx), 0, "a pending call draws no row");
    result(cx, w, &view, "toolu_l1", &put);
    assert!(has(cx, w, format!("block-rev {live} status · r1")), "bound: r1");
    assert_eq!(tool_rows(cx), 0);
    // Refused: a plain row, the fail-safe.
    let bad = json!([{"op":"put","id":"x","kind":"status","body":{"text":"x","state":"nope"}}]);
    heard(
        cx,
        w,
        &view,
        vec![
            llm(json!({"kind": "llm", "llm": "response_start"})),
            llm(json!({"kind": "llm", "llm": "tool_use", "tool_name": "cards_block", "tool_id": "toolu_l2", "input": block_input(bad)})),
            llm(
                json!({"kind": "llm", "llm": "tool_result", "tool_id": "toolu_l2", "result": "cards_block: ops[0].body.state: \"nope\" is not a status state"}),
            ),
        ],
    );
    board(cx, w, &view);
    let plain = view.read_with(cx, |v, _| {
        let items = v.model.lives[v.panes.cur.live].shown().items();
        v.panes.cur.disp.iter().any(|d| d.kind == Kind::Work && items[d.item].tool.as_ref().is_some_and(|t| t.tool_id == "toolu_l2"))
    });
    assert!(plain, "a refused call is a plain row");
}

/// Live and the history draw the same (§1.7): the live turn's blocks, then
/// the same turn landed as history, draw the same rows; nothing blinks
/// between (the last board draws until the next lands).
#[gpui_kit::test]
fn live_and_landed_draw_the_same_rows(cx: &mut TestAppContext) {
    let (w, view) = blocks(cx, 1500.);
    turn(cx, w, &view);
    let put = json!([{"op":"put","id":"live","kind":"plan","body":{"steps":[{"id":"a","label":"A","state":"doing"}]}}]);
    round(cx, w, &view, &[("toolu_l1", put.clone(), true)]);
    let rows = |cx: &mut TestAppContext| {
        view.read_with(cx, |v, _| {
            v.panes.cur.disp.iter().filter(|d| matches!(d.kind, Kind::Block { .. })).map(|d| d.id).collect::<Vec<_>>()
        })
    };
    let live = rows(cx);
    land(
        cx,
        w,
        &view,
        vec![
            json!({"role": "user", "content": "go on"}),
            json!({"role": "assistant", "content": [{"type": "tool_use", "id": "toolu_l1", "name": "cards_block", "input": block_input(put.clone())}]}),
            json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_l1", "content": block_receipt(&put)}]}),
        ],
    );
    assert_eq!(rows(cx), live, "landed, before its board: the last board's rows");
    board(cx, w, &view);
    assert_eq!(rows(cx), live, "landed: the same rows");
}

/// Above the view (§6.2): a block above it that grows keeps every row in
/// view where it was (the list keeps its anchor), and the pill says so;
/// a click on the pill walks to it, and the pill goes.
#[gpui_kit::test]
fn a_change_above_the_view_keeps_it_and_the_pill_walks_to_it(cx: &mut TestAppContext) {
    let (w, view) = blocks(cx, 1500.);
    turn(cx, w, &view);
    // Rows under the plan fill the view: the plan is above it.
    let filler: Vec<Value> = (0..14).map(|k| json!({"op":"put","id":format!("f{k}"),"kind":"note","body":{"text":"filler"}})).collect();
    round(cx, w, &view, &[("toolu_f", json!(filler), true)]);
    view.update(cx, |v, cx| {
        v.panes.cur.list.scroll_to_end();
        cx.notify();
    });
    settle(cx, w, 8);
    let plan = id(cx, &view, "plan");
    assert!(!has(cx, w, format!("block {plan}")), "the plan is above the view");
    let tops = super::row_tops(cx, &view);
    let grow = json!([{"op":"patch","id":"plan","append":{"steps":[{"id":"more","label":"One more","state":"todo"}]}}]);
    round(cx, w, &view, &[("toolu_g", grow, true)]);
    let after = super::row_tops(cx, &view);
    for (k, top) in &tops {
        if let Some(t) = after.get(k) {
            assert!((t - top).abs() < 0.5, "row {k:?} stayed: {top} → {t}");
        }
    }
    assert!(has(cx, w, "blocks-above 0".into()), "the pill");
    super::answer_tests::click(cx, w, &view, "blocks-above 0".into());
    assert!(has(cx, w, format!("block {plan}")), "walked to the plan");
    settle(cx, w, 8);
    assert!(!has(cx, w, "blocks-above 0".into()), "the pill goes once it is in view");
}

/// The pointer lease (§6.2): a moving pointer over block B; block A above
/// it grows: the board waits, B stays where it is; the lease's end (one
/// wake, armed only while a change waits) draws it. B replaced while its
/// old view is painted: a click on it is a no-op with the note, never on
/// the new B. B removed: waits too. With nothing waiting, no wake.
#[gpui_kit::test]
fn the_lease_holds_what_would_move_under_the_pointer(cx: &mut TestAppContext) {
    let (w, view) = blocks(cx, 1500.);
    let put = json!([
        {"op":"put","id":"a","kind":"plan","body":{"steps":[{"id":"s1","label":"One","state":"todo"}]}},
        {"op":"put","id":"b","kind":"status","body":{"text":"B","state":"working"}}
    ]);
    put_mid(cx, w, &view, put);
    let b = id(cx, &view, "b");
    let at = head_at(cx, w, b);
    mouse(cx, w, at, "move");
    assert!(view.read_with(cx, |v, _| v.lease_wake.is_none()), "nothing waits: no wake");
    let y = |cx: &mut TestAppContext| f32::from(bounds(cx, w, format!("block {b}")).expect("B drawn").top());
    let y0 = y(cx);
    // A grows: it waits.
    let grow = json!([{"op":"patch","id":"a","append":{"steps":[{"id":"s2","label":"Two","state":"todo"}]}}]);
    round(cx, w, &view, &[("toolu_g", grow, true)]);
    assert_eq!(y(cx), y0, "B stays under the pointer");
    assert!(view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].board.waiting.is_some()), "the board waits");
    assert!(view.read_with(cx, |v, _| v.lease_wake.is_some()), "one wake, armed");
    // An in-place change alone would draw at once; the lease ends: all of it draws.
    wait(cx, w, &view, 1600);
    board(cx, w, &view);
    assert!(view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].board.waiting.is_none()), "drawn at the lease's end");
    // B slides to its new place (§6.3), then is there.
    wait(cx, w, &view, 1000);
    assert!(y(cx) > y0, "A grew: B moved once the pointer rested");
    assert!(view.read_with(cx, |v, _| v.lease_wake.is_none()), "the wake spent");
    // B replaced while its old view is painted: the click is a no-op
    // (past B's just-moved window, which the release opened).
    wait(cx, w, &view, 700);
    let at = head_at(cx, w, b);
    mouse(cx, w, at, "move");
    let replace = json!([{"op":"replace","id":"b","kind":"status","body":{"text":"B again","state":"working"}}]);
    round(cx, w, &view, &[("toolu_r", replace, true)]);
    assert!(view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].board.waiting.is_some()), "a replace waits");
    let was = folded(cx, &view, b);
    mouse(cx, w, at, "down");
    mouse(cx, w, at, "up");
    assert_eq!(folded(cx, &view, b), was, "the click did nothing");
    assert_eq!(note(cx, &view), Some(crate::lease::CHANGED));
    // The pointer leaves the list: what waited draws.
    mouse(cx, w, gpui_kit::point(px(2.), px(500.)), "move");
    board(cx, w, &view);
    assert!(view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].board.waiting.is_none()));
    // B removed under a moving pointer: waits too.
    let at = head_at(cx, w, b);
    mouse(cx, w, at, "move");
    round(cx, w, &view, &[("toolu_x", json!([{"op":"remove","id":"b"}]), true)]);
    assert!(view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].board.waiting.is_some()), "a remove waits");
    assert!(has(cx, w, format!("block {b}")), "the old geometry stays drawn");
    wait(cx, w, &view, 1600);
    board(cx, w, &view);
    assert!(!has(cx, w, format!("block {b}")), "gone once the lease ends");
}

/// An in-place change under the lease draws at once: a tick keeps the
/// structure.
#[gpui_kit::test]
fn an_in_place_change_draws_under_the_lease(cx: &mut TestAppContext) {
    let (w, view) = blocks(cx, 1500.);
    turn(cx, w, &view);
    let put = json!([{"op":"put","id":"b","kind":"plan","body":{"steps":[{"id":"s1","label":"One","state":"todo"},{"id":"s2","label":"Two","state":"todo"}]}}]);
    round(cx, w, &view, &[("toolu_p", put, true)]);
    view.update(cx, |v, cx| {
        v.panes.cur.list.scroll_to_end();
        cx.notify();
    });
    settle(cx, w, 8);
    let b = id(cx, &view, "b");
    let at = head_at(cx, w, b);
    mouse(cx, w, at, "move");
    round(cx, w, &view, &[("toolu_t", json!([{"op":"patch","id":"b","state":{"s1":"done"}}]), true)]);
    assert!(view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].board.waiting.is_none()), "a tick draws at once");
    assert!(has(cx, w, format!("block-word {b} 1/2")));
}

/// Just moved (§6.2): a resting pointer's click 300 ms after an agent op
/// moved its target does nothing, once (never replayed); 700 ms after, it
/// acts; 300 ms after an op moved something else, it acts. The user's own
/// fold never starts the window.
#[gpui_kit::test]
fn a_click_on_a_target_that_just_moved_does_nothing_once(cx: &mut TestAppContext) {
    let (w, view) = blocks(cx, 1500.);
    let put = json!([
        {"op":"put","id":"a","kind":"plan","body":{"steps":[{"id":"s1","label":"One","state":"todo"}]}},
        {"op":"put","id":"b","kind":"note","body":{"text":"B"}},
        {"op":"put","id":"c","kind":"note","body":{"text":"C"}}
    ]);
    put_mid(cx, w, &view, put);
    let (b, c) = (id(cx, &view, "b"), id(cx, &view, "c"));
    let click = |cx: &mut TestAppContext, row: u64| {
        let at = head_at(cx, w, row);
        mouse(cx, w, at, "down");
        mouse(cx, w, at, "up");
    };
    // A grows (the pointer resting, no lease): B and C move down.
    let grow = |k: &str| json!([{"op":"patch","id":"a","append":{"steps":[{"id":k,"label":k,"state":"todo"}]}}]);
    round(cx, w, &view, &[("toolu_g1", grow("s2"), true)]);
    wait(cx, w, &view, 300);
    let was = folded(cx, &view, b);
    click(cx, b);
    assert_eq!(folded(cx, &view, b), was, "300 ms after it moved: nothing");
    assert_eq!(note(cx, &view), Some(crate::lease::MOVED));
    click(cx, b);
    assert_ne!(folded(cx, &view, b), was, "once: the next click acts");
    round(cx, w, &view, &[("toolu_g2", grow("s3"), true)]);
    wait(cx, w, &view, 700);
    let was = folded(cx, &view, c);
    click(cx, c);
    assert_ne!(folded(cx, &view, c), was, "700 ms after: it acts");
    // An op that moves only what is under the target: the target acts.
    let under = json!([{"op":"patch","id":"c","set":{"text":"C\nC\nC"}}]);
    round(cx, w, &view, &[("toolu_d", under, true)]);
    wait(cx, w, &view, 300);
    let a = id(cx, &view, "a");
    let was = folded(cx, &view, a);
    click(cx, a);
    assert_ne!(folded(cx, &view, a), was, "300 ms after an op moved something else: it acts");
    // The user's own fold moves what is under it: those act at once.
    wait(cx, w, &view, 1000);
    click(cx, a);
    let was = folded(cx, &view, c);
    click(cx, c);
    assert_ne!(folded(cx, &view, c), was, "the user's own change never starts the window");
}

/// Down and up (§6.2): a press held 2 s while an agent op waits acts on
/// what it pressed, and the op draws at its release.
#[gpui_kit::test]
fn a_held_press_acts_on_what_it_pressed(cx: &mut TestAppContext) {
    let (w, view) = blocks(cx, 1500.);
    turn(cx, w, &view);
    let put = json!([
        {"op":"put","id":"a","kind":"plan","body":{"steps":[{"id":"s1","label":"One","state":"todo"}]}},
        {"op":"put","id":"b","kind":"note","body":{"text":"B"}}
    ]);
    round(cx, w, &view, &[("toolu_p", put, true)]);
    view.update(cx, |v, cx| {
        v.panes.cur.list.scroll_to_end();
        cx.notify();
    });
    settle(cx, w, 8);
    wait(cx, w, &view, 1000);
    let b = id(cx, &view, "b");
    let at = head_at(cx, w, b);
    mouse(cx, w, at, "move");
    mouse(cx, w, at, "down");
    let grow = json!([{"op":"patch","id":"a","append":{"steps":[{"id":"s2","label":"Two","state":"todo"}]}}]);
    round(cx, w, &view, &[("toolu_g", grow, true)]);
    wait(cx, w, &view, 2000);
    assert!(view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].board.waiting.is_some()), "held while pressed");
    let was = folded(cx, &view, b);
    mouse(cx, w, at, "up");
    assert_ne!(folded(cx, &view, b), was, "it acts on what it pressed");
    wait(cx, w, &view, 1600);
    board(cx, w, &view);
    assert!(view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].board.waiting.is_none()), "drawn at release");
}

/// A key releases the lease: what waited draws.
#[gpui_kit::test]
fn a_key_releases_the_lease(cx: &mut TestAppContext) {
    let (w, view) = blocks(cx, 1500.);
    turn(cx, w, &view);
    let put = json!([
        {"op":"put","id":"a","kind":"plan","body":{"steps":[{"id":"s1","label":"One","state":"todo"}]}},
        {"op":"put","id":"b","kind":"note","body":{"text":"B"}}
    ]);
    round(cx, w, &view, &[("toolu_p", put, true)]);
    view.update(cx, |v, cx| {
        v.panes.cur.list.scroll_to_end();
        cx.notify();
    });
    settle(cx, w, 8);
    let b = id(cx, &view, "b");
    let at = head_at(cx, w, b);
    mouse(cx, w, at, "move");
    let grow = json!([{"op":"patch","id":"a","append":{"steps":[{"id":"s2","label":"Two","state":"todo"}]}}]);
    round(cx, w, &view, &[("toolu_g", grow, true)]);
    assert!(view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].board.waiting.is_some()));
    super::answer_tests::key(cx, w, "down");
    board(cx, w, &view);
    assert!(view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].board.waiting.is_none()), "a key released it");
}

/// Motion (§6.3): a change once bound asks for frames only for its glow
/// (480 ms; Reduced a 120 ms flash; Off none), and none after.
#[gpui_kit::test]
fn a_glow_asks_for_frames_only_while_it_runs(cx: &mut TestAppContext) {
    for level in [crate::motion::Level::Full, crate::motion::Level::Reduced, crate::motion::Level::Off] {
        let (w, view) = blocks(cx, 1500.);
        motion_level(cx, &view, level);
        let call = |tool: &str, ops: &Value| {
            vec![
                json!({"role": "assistant", "content": [{"type": "tool_use", "id": tool, "name": "cards_block", "input": block_input(ops.clone())}]}),
                json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": tool, "content": block_receipt(ops)}]}),
            ]
        };
        land(cx, w, &view, call("toolu_p", &json!([{"op":"put","id":"b","kind":"status","body":{"text":"one","state":"waiting"}}])));
        board(cx, w, &view);
        view.update(cx, |v, cx| {
            v.panes.cur.list.scroll_to_end();
            cx.notify();
        });
        wait(cx, w, &view, 2000);
        let frames = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.fx.frames());
        let f0 = frames(cx);
        land(cx, w, &view, call("toolu_t", &json!([{"op":"patch","id":"b","set":{"text":"two"}}])));
        board(cx, w, &view);
        let b = id(cx, &view, "b");
        assert!(has(cx, w, format!("block-rev {b} status · r2")));
        wait(cx, w, &view, 600);
        let during = frames(cx) - f0;
        let f1 = frames(cx);
        wait(cx, w, &view, 2000);
        assert_eq!(frames(cx) - f1, 0, "{level:?}: no frames after the glow");
        if level == crate::motion::Level::Full {
            assert!(during >= 3, "Full: the afterglow's steps drew ({during} frames)");
        }
    }
}

/// Re-folds (`cards_spec::board` guarantee 6): one at a time, off the
/// frame; the board at a position is the reference's; a request while one
/// runs stops it and waits for its end; another history drops it; asked
/// again once done, nothing runs; withdrawn, nothing is kept.
#[gpui_kit::test]
fn refolds_run_one_at_a_time_and_drop_when_their_history_changes(cx: &mut TestAppContext) {
    use cards_spec::board::{Pos, Refold, Target};
    let (w, view) = blocks(cx, 1500.);
    let live = view.read_with(cx, |v, _| v.panes.cur.live);
    let (events, generation) = view.read_with(cx, |v, _| {
        let i = v.model.lives[live].transcript.blocks_input().expect("blocks");
        (i.events(), i.generation)
    });
    let want = |at: Target| {
        let mut rf = Refold::to(generation, &events, at).unwrap();
        while !rf.step(usize::MAX).unwrap() {}
        rf.state().cloned()
    };
    let (a, b) = (Target::Revision(Pos::event(8)), Target::Revision(Pos::event(events.len() - 1)));
    let done = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.refolds.done.clone());
    let started = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.refolds.started);
    view.update(cx, |v, cx| v.refold(live, a, cx));
    // Another request before it ends: it stops, and B runs after it.
    view.update(cx, |v, cx| v.refold(live, b, cx));
    assert_eq!(started(cx), 1, "one at a time: B waits");
    cx.run_until_parked();
    let d = done(cx).expect("B finished");
    assert_eq!(d.target, b, "A was dropped, never finished");
    assert_eq!(started(cx), 2);
    assert_eq!(d.state.clone().unwrap().map(|s| (*s).clone()), want(b));
    assert!(d.visited >= events.len());
    // Asked again: nothing runs.
    view.update(cx, |v, cx| v.refold(live, b, cx));
    cx.run_until_parked();
    assert_eq!(started(cx), 2);
    // A on its own: the board at event 8.
    view.update(cx, |v, cx| v.refold(live, a, cx));
    cx.run_until_parked();
    assert_eq!(started(cx), 3);
    let d = done(cx).expect("A finished");
    assert_eq!((d.target, d.state.unwrap().map(|s| (*s).clone())), (a, want(a)));
    // Withdrawn: nothing kept.
    view.update(cx, |v, _| v.refold_cancel());
    assert!(done(cx).is_none());
    // Another history while it runs (before it has had a turn): dropped,
    // not finished.
    let epoch = view.read_with(cx, |v, _| v.model.lives[live].transcript.blocks_epoch());
    view.update(cx, |v, cx| {
        v.refold(live, b, cx);
        v.model.lives[live].transcript.apply_history(&synaps_client::synthetic::blocks_history()[..4]);
        v.sync_board(live, cx);
    });
    assert_ne!(view.read_with(cx, |v, _| v.model.lives[live].transcript.blocks_epoch()), epoch, "another history");
    cx.run_until_parked();
    settle(cx, w, 8);
    assert!(done(cx).is_none(), "a re-fold of another history is dropped");
    assert_eq!(started(cx), 4, "A, B, A again, then B on the old history");
}

/// Bounded caches (§1.8: renderer caches per pane): a pane's state per
/// block (its fold, a well's scroll, `more`, its painted target) keeps
/// only the blocks the board draws, and its edit previews one per file
/// at most.
#[gpui_kit::test]
fn a_panes_block_caches_keep_only_what_the_board_draws(cx: &mut TestAppContext) {
    let (w, view) = blocks(cx, 1500.);
    let why = show_block(cx, w, &view, "why");
    let at = head_at(cx, w, why);
    mouse(cx, w, at, "move");
    mouse(cx, w, at, "down");
    mouse(cx, w, at, "up");
    assert!(!folded(cx, &view, why));
    let kept = |cx: &mut TestAppContext| {
        view.read_with(cx, |v, _| {
            let p = &v.panes.cur;
            let mut rows: Vec<u64> = p.block_folds.rows();
            rows.extend(p.lease.painted.borrow().keys());
            rows.extend(p.block_scroll.borrow().keys());
            rows.extend(p.block_more.iter());
            rows
        })
    };
    assert!(kept(cx).contains(&why), "its fold and its painted target kept");
    // The pointer off the list (no lease), then the group removed.
    mouse(cx, w, gpui_kit::point(px(2.), px(500.)), "move");
    turn(cx, w, &view);
    round(cx, w, &view, &[("toolu_x", json!([{"op":"remove","id":"why"}]), true)]);
    assert!(row(cx, &view, "why").is_none(), "removed");
    let drawn: std::collections::HashSet<u64> = view.read_with(cx, |v, _| {
        let d = v.model.lives[v.panes.cur.live].transcript.drawn().cloned().expect("a board");
        d.rows
            .values()
            .flat_map(|r| r.blocks.iter().flat_map(|s| std::iter::once(s).chain(&s.members)))
            .map(|s| crate::blocks::row_id(&s.block.id))
            .collect()
    });
    let left: Vec<u64> = kept(cx).into_iter().filter(|r| !drawn.contains(r)).collect();
    assert!(left.is_empty(), "kept for blocks not drawn: {left:?}");
    let files = view.read_with(cx, |v, _| v.panes.cur.block_edits.borrow().len());
    assert!(files <= 32, "{files} edit previews");
}

/// A move within view slides (§6.3): at Full, a block an agent op moved
/// (here: the block above it moved away) starts where it was painted and
/// settles on its new place, at most [`crate::motion::SLIDE_CAP`] from
/// it. Reduced and Off move it at once, and so does a list following its
/// tail (its place on screen says nothing about a move).
#[gpui_kit::test]
fn a_move_within_view_slides_at_full_only(cx: &mut TestAppContext) {
    use crate::motion::Level;
    for (level, follow) in [(Level::Full, false), (Level::Reduced, false), (Level::Off, false), (Level::Full, true)] {
        let (w, view) = blocks(cx, 1500.);
        motion_level(cx, &view, level);
        // X at the view's top keeps it (the list's anchor is its top row).
        let put = json!([
            {"op":"put","id":"x","kind":"note","body":{"text":"X"}},
            {"op":"put","id":"a","kind":"note","body":{"text":"A"}},
            {"op":"put","id":"b","kind":"note","body":{"text":"B"}},
            {"op":"put","id":"c","kind":"note","body":{"text":"C"}}
        ]);
        if follow {
            turn(cx, w, &view);
            round(cx, w, &view, &[("toolu_p", put, true)]);
            view.update(cx, |v, cx| {
                v.panes.cur.list.scroll_to_end();
                cx.notify();
            });
            wait(cx, w, &view, 1000);
            assert!(view.read_with(cx, |v, _| v.panes.cur.list.is_following_tail()), "following its tail");
        } else {
            put_mid(cx, w, &view, put);
            assert!(!view.read_with(cx, |v, _| v.panes.cur.list.is_following_tail()), "anchored");
        }
        let b = id(cx, &view, "b");
        let top = |cx: &mut TestAppContext| {
            let rows = view.read_with(cx, |v, _| v.panes.cur.disp.iter().filter(|d| matches!(d.kind, Kind::Block { .. })).count());
            f32::from(
                bounds(cx, w, format!("block {b}")).unwrap_or_else(|| panic!("{level:?} {follow}: B drawn ({rows} block rows)")).top(),
            )
        };
        let y0 = top(cx);
        round(cx, w, &view, &[("toolu_m", json!([{"op":"move","id":"a"}]), true)]);
        let mid = top(cx);
        wait(cx, w, &view, 1000);
        let y1 = top(cx);
        assert!(y1 < y0 - 20., "{level:?} {follow}: B moved up ({y0} → {y1})");
        if level == Level::Full && !follow {
            assert!(mid > y1 + 0.5 && mid <= y0 + 0.5, "Full: B starts where it was ({y0} → {mid} → {y1})");
            assert!(mid - y1 <= crate::motion::SLIDE_CAP + 0.5, "at most the cap from its place");
            assert_eq!(settle(cx, w, 8), 0, "settled: no frames asked");
        } else {
            assert!((mid - y1).abs() < 0.5, "{level:?} {follow}: at once ({y0} → {mid} → {y1})");
        }
    }
}
