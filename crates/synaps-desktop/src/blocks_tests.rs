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
fn every_kind_draws_at_its_heights_narrow_and_wide(cx: &mut TestAppContext) {
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

/// The daemon detaches a session its tab still shows (it ended, or was
/// taken over): the transcript stays, read-only, and so does its board
/// work (Shady #101 P2-4: release what no pane owns; this pane owns it).
/// The board drawn stays drawn, the history's fold stays with it, and a
/// board that waited under the pointer lease (which the detach cleared)
/// still draws, the newest; nothing runs after. Its fold goes with the
/// transcript (a switch away, a close: `a_sessions_block_state_and_board_
/// work_go_with_its_transcript`).
#[gpui_kit::test]
fn a_detached_sessions_board_stays_with_the_transcript_it_shows(cx: &mut TestAppContext) {
    let (w, view) = blocks(cx, 1500.);
    let live = view.read_with(cx, |v, _| v.panes.cur.live);
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
    assert!(view.read_with(cx, |v, _| v.model.lives[live].board.waiting.is_some()), "a board waits under the lease");
    let (seq, sid) = super::answer_tests::attachment(cx, &view);
    view.update(cx, |v, cx| {
        let ev = synaps_client::Event::Detached { attach_seq: seq, session_id: Some(sid), reason: "session ended".into() };
        v.apply_batch(vec![ev], cx);
    });
    let state = |cx: &mut TestAppContext| {
        view.read_with(cx, |v, _| {
            let l = &v.model.lives[live];
            (l.active.is_none(), l.transcript.drawn().is_some(), l.transcript.blocks_retained().journal > 0)
        })
    };
    assert_eq!(state(cx), (true, true, true), "detached: its board drawn and its fold kept");
    wait(cx, w, &view, 2000);
    board(cx, w, &view);
    let steps = view.read_with(cx, |v, _| {
        let (_, s) = v.block_shown(row_of(v, "a")).expect("a drawn");
        match &s.block.body {
            synaps_client::blocks::Body::Plan(p) => p.steps.len(),
            _ => 0,
        }
    });
    assert_eq!(steps, 2, "the board that waited draws: the newest");
    view.read_with(cx, |v, _| {
        let l = &v.model.lives[live];
        assert!(l.board.waiting.is_none() && !l.board.running(), "nothing waits or runs after");
    });
    assert_eq!(state(cx), (true, true, true), "still drawn, its fold still kept");
    assert!(row(cx, &view, "b").is_some(), "the read-only transcript's blocks");
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

/// Motion keeps to its level from the first frame (§6.3, decision #3;
/// Shady #101 P3-1: a three-second quiet window can't tell a Reduced glow
/// stretched to Full's from Reduced's). A bound change of a status's text
/// and a plan step's state, sampled on the frames after it publishes: the
/// text's afterglow is lit at Full through 400 ms and out by 600 (GLOW,
/// 480 ms), at Reduced lit for its flash only (QUICK, 120 ms: out by 200),
/// at Off never; the step's glyph turns (`life-turn`, inside its row) at
/// Full in its first 132 ms (two TURNs) and is over by 200, and at Reduced
/// and Off never turns at all.
#[gpui_kit::test]
fn a_glow_and_a_glyph_turn_keep_to_their_level_from_the_first_frame(cx: &mut TestAppContext) {
    use crate::motion::Level;
    for level in [Level::Full, Level::Reduced, Level::Off] {
        let (w, view) = blocks(cx, 1500.);
        motion_level(cx, &view, level);
        let put = json!([
            {"op":"put","id":"g","kind":"status","body":{"text":"one","state":"working"}},
            {"op":"put","id":"q","kind":"plan","body":{"steps":[{"id":"s","label":"Step","state":"doing"}]}}
        ]);
        land_round(cx, w, &view, "toolu_gp", put);
        view.update(cx, |v, cx| {
            v.panes.cur.list.scroll_to_end();
            cx.notify();
        });
        wait(cx, w, &view, 2000);
        let (g, q) = (id(cx, &view, "g"), id(cx, &view, "q"));
        assert!(has(cx, w, format!("block-text {g} rest")), "{level:?}: at rest");
        let tick = json!([{"op":"patch","id":"g","set":{"text":"two"}},{"op":"patch","id":"q","state":{"s":"done"}}]);
        land_round(cx, w, &view, "toolu_gt", tick);
        let sample = |cx: &mut TestAppContext| {
            let lit = has(cx, w, format!("block-text {g} lit"));
            let step = bounds(cx, w, format!("block-step {q} s")).expect("the step drawn");
            let turning = (0..3).filter_map(|n| bounds(cx, w, format!("life-turn {n}"))).any(|b| step.contains(&b.center()));
            (lit, turning)
        };
        let mut seen = vec![sample(cx)];
        for ms in [60, 140, 200, 200] {
            wait(cx, w, &view, ms);
            seen.push(sample(cx));
        }
        let (lit, turn): (Vec<bool>, Vec<bool>) = seen.into_iter().unzip();
        // At 0, 60, 200, 400 and 600 ms.
        let (want_lit, want_turn) = match level {
            Level::Full => ([true, true, true, true, false], [true, true, false, false, false]),
            Level::Reduced => ([true, true, false, false, false], [false; 5]),
            Level::Off => ([false; 5], [false; 5]),
        };
        assert_eq!(lit, want_lit, "{level:?}: the text's afterglow at 0, 60, 200, 400, 600 ms");
        assert_eq!(turn, want_turn, "{level:?}: the step's glyph turning at 0, 60, 200, 400, 600 ms");
    }
}

/// A user's local fold is theirs until the agent folds or opens that
/// block by name (decision #4, §3.5; B2's `folded_at` is the last
/// `collapse` naming it). A step's state, a title, a replace, a collapse
/// of another block and a run's title leave it; a `collapse` naming it
/// (`open`, or among `ids`) overrides it, and the user's next fold is
/// theirs again.
#[gpui_kit::test]
fn a_local_fold_yields_only_to_a_collapse_naming_that_block(cx: &mut TestAppContext) {
    let (w, view) = blocks(cx, 1500.);
    let toggle = |cx: &mut TestAppContext, row: u64| {
        let (ix, _) = view.read_with(cx, |v, _| v.block_shown(row)).expect("drawn");
        show(cx, w, &view, ix);
        wait(cx, w, &view, 1000);
        let at = head_at(cx, w, row);
        mouse(cx, w, at, "move");
        mouse(cx, w, at, "down");
        mouse(cx, w, at, "up");
        board(cx, w, &view);
        mouse(cx, w, gpui_kit::point(px(2.), px(2.)), "move");
    };
    let plan = id(cx, &view, "plan");
    assert!(!folded(cx, &view, plan), "the agent never folded it");
    toggle(cx, plan);
    assert!(folded(cx, &view, plan), "the user's fold");
    let ticks = [
        json!([{"op":"patch","id":"plan","state":{"deploy":"doing"}}]),
        json!([{"op":"patch","id":"plan","set":{"title":"Close the race"}}]),
        json!([{"op":"replace","id":"plan","kind":"plan","title":"Close it","body":{"steps":[{"id":"z","label":"Z","state":"todo"}]}}]),
        json!([{"op":"collapse","id":"now"}]),
        json!([{"op":"collapse","run":true,"summary":"Another run"}]),
    ];
    for (k, ops) in ticks.into_iter().enumerate() {
        land_round(cx, w, &view, &format!("toolu_f{k}"), ops.clone());
        wait(cx, w, &view, 1000);
        assert!(folded(cx, &view, plan), "{ops}: the user's fold stays");
    }
    land_round(cx, w, &view, "toolu_fo", json!([{"op":"collapse","id":"plan","open":true}]));
    wait(cx, w, &view, 1000);
    assert!(!folded(cx, &view, plan), "a collapse naming it, open: the agent's");
    toggle(cx, plan);
    assert!(folded(cx, &view, plan), "the user's again");
    land_round(cx, w, &view, "toolu_fp", json!([{"op":"patch","id":"plan","state":{"z":"done"}}]));
    wait(cx, w, &view, 1000);
    assert!(folded(cx, &view, plan), "a tick leaves it");
    toggle(cx, plan);
    land_round(cx, w, &view, "toolu_fi", json!([{"op":"collapse","ids":["plan","now"]}]));
    wait(cx, w, &view, 1000);
    assert!(folded(cx, &view, plan), "named among ids: the agent's fold over the user's opening");
}

/// Every state draws its own glyph and word, at 320 px and wide (§6.1,
/// §6.4; Shady #101 P3-1: the demo doesn't hold every state, so a broken
/// branch it never reaches stayed green). A block per agent state, each
/// header's glyph and word as the table says (at 320 px the word stays:
/// it gives way below 250); a plan per step state, its glyph the steps'
/// aggregate; and a diff's hunks kept and dropped by a genuine user edit
/// in the history (§3.4: the user's, never the agent's), each hunk's
/// header saying so.
#[gpui_kit::test]
fn every_state_draws_its_glyph_and_word_narrow_and_wide(cx: &mut TestAppContext) {
    use crate::glyph::Life;
    let table: &[(&str, &str, Value, Life, Option<&str>)] = &[
        ("s1", "status", json!({"text":"t","state":"working"}), Life::Seen, Some("working")),
        ("s2", "status", json!({"text":"t","state":"waiting"}), Life::Wait, Some("waiting")),
        ("s3", "status", json!({"text":"t","state":"blocked"}), Life::Failed, Some("blocked")),
        ("s4", "status", json!({"text":"t","state":"done"}), Life::Done, Some("done")),
        ("d1", "diff", json!({"file":"a.rs","state":"editing","hunks":[{"id":"h","lines":["+a"]}]}), Life::Seen, Some("editing")),
        ("d2", "diff", json!({"file":"a.rs","state":"proposed","hunks":[{"id":"h","lines":["+a"]}]}), Life::Wait, Some("proposed")),
        ("d3", "diff", json!({"file":"a.rs","state":"written","hunks":[{"id":"h","lines":["+a"]}]}), Life::Done, Some("written")),
        ("d4", "diff", json!({"file":"a.rs","state":"reverted","hunks":[{"id":"h","lines":["+a"]}]}), Life::Cancelled, Some("reverted")),
        ("m1", "metric", json!({"label":"l","value":"1","state":"ok"}), Life::Done, Some("ok")),
        ("m2", "metric", json!({"label":"l","value":"1","state":"warn"}), Life::Wait, Some("warn")),
        ("m3", "metric", json!({"label":"l","value":"1","state":"bad"}), Life::Failed, Some("bad")),
        ("m4", "metric", json!({"label":"l","value":"1","state":"neutral"}), Life::Queued, None),
        ("f1", "file", json!({"path":"a.rs","state":"reading"}), Life::Seen, Some("reading")),
        ("f2", "file", json!({"path":"a.rs","state":"editing"}), Life::Seen, Some("editing")),
        ("f3", "file", json!({"path":"a.rs","state":"written"}), Life::Done, Some("written")),
        ("f4", "file", json!({"path":"a.rs","state":"done"}), Life::Done, Some("done")),
        ("p1", "plan", json!({"steps":[{"id":"a","label":"A","state":"todo"}]}), Life::Queued, None),
        ("p2", "plan", json!({"steps":[{"id":"a","label":"A","state":"doing"}]}), Life::Seen, None),
        ("p3", "plan", json!({"steps":[{"id":"a","label":"A","state":"done"}]}), Life::Done, None),
        ("p4", "plan", json!({"steps":[{"id":"a","label":"A","state":"failed"}]}), Life::Failed, None),
        ("p5", "plan", json!({"steps":[{"id":"a","label":"A","state":"skipped"}]}), Life::Done, None),
        ("p6", "plan", json!({"steps":[{"id":"a","label":"A","state":"done"},{"id":"b","label":"B","state":"doing"}]}), Life::Seen, None),
        ("n1", "note", json!({"text":"a note"}), Life::Done, None),
    ];
    for width in [320. + 2. * crate::app::STRIP, 1500.] {
        let (w, view) = blocks(cx, width);
        // Eight at a time (a call's ops and the board's blocks are capped),
        // each eight removed once read.
        for (k, chunk) in table.chunks(8).enumerate() {
            let ops: Vec<Value> =
                chunk.iter().map(|(id, kind, body, ..)| json!({"op":"put","id":id,"kind":kind,"title":id,"body":body})).collect();
            land_round(cx, w, &view, &format!("toolu_st{k}"), Value::Array(ops));
            for (name, kind, _, life, word) in chunk {
                let id = show_block(cx, w, &view, name);
                assert!(has(cx, w, format!("block-life {id} {life:?}")), "{width}: {name} ({kind}): its glyph, {life:?}");
                assert!(has(cx, w, format!("block-title {id}")), "{width}: {name}: its title");
                for other in
                    ["working", "waiting", "blocked", "done", "editing", "proposed", "written", "reverted", "ok", "warn", "bad", "reading"]
                {
                    let drawn = has(cx, w, format!("block-word {id} {other}"));
                    assert_eq!(drawn, *word == Some(other), "{width}: {name} ({kind}): the word {other}");
                }
            }
            let ids: Vec<&str> = chunk.iter().map(|c| c.0).collect();
            land_round(cx, w, &view, &format!("toolu_sr{k}"), json!([{"op":"remove","ids":ids}]));
        }
        // Hunks: the user keeps one and drops one (a genuine edit message).
        let diff = json!({"file":"a.rs","state":"proposed","hunks":[
            {"id":"h1","at":"one()","lines":["+a"]},{"id":"h2","at":"two()","lines":["-b"]},{"id":"h3","at":"three()","lines":["+c"]}]});
        land_round(cx, w, &view, "toolu_hk", json!([{"op":"put","id":"hk","kind":"diff","title":"Hunks","body":diff}]));
        let hk = id(cx, &view, "hk");
        let (rf, rev, tag) = view.read_with(cx, |v, _| {
            let (_, s) = v.block_shown(hk).expect("drawn");
            (s.block.id.clone(), s.block.rev, s.block.tag)
        });
        let parts = json!({"set": {"hunks.h1.state": "kept", "hunks.h2.state": "dropped"}});
        let edit = cards_spec::block::message::Edit::new(rf, rev, tag, &parts).expect("a user edit");
        let summary = cards_spec::block::message::Summary { title: Some("Hunks"), back_to: None, pieces: vec!["keep one()", "drop two()"] };
        let text = cards_spec::block::message::build(&edit, &summary).expect("built");
        land(cx, w, &view, vec![json!({"role": "user", "content": text})]);
        board(cx, w, &view);
        show_block(cx, w, &view, "hk");
        for line in ["@@ one() · kept", "@@ two() · dropped", "@@ three()"] {
            assert!(has(cx, w, format!("block-line {hk} {line}")), "{width}: {line}");
        }
    }
}

/// Only a finished board is published, and only the input's snapshot it
/// ends on (Shady #101 P3-1: the worker returning a whole value is not
/// the app publishing one). A long history lands (its fold: many slices, a
/// step each here) and the job runs a step at a time on the executor; in
/// between, frames draw and the old board stays drawn, nothing published.
/// A live round lands mid-fold: the job for the old input stops at its
/// next step, and the next one reads on. When it ends, one board is
/// published, of the input as it is then (its history and live events,
/// the live round's block drawn), and nothing else ever was.
#[gpui_kit::test]
fn a_job_paused_mid_fold_publishes_only_the_snapshot_it_ends_on(cx: &mut TestAppContext) {
    let (w, view) = blocks(cx, 1500.);
    let live = view.read_with(cx, |v, _| v.panes.cur.live);
    view.update(cx, |v, _| v.model.lives[live].board.budget = Some(std::time::Duration::ZERO));
    let frame = |cx: &mut TestAppContext| {
        use gpui_kit::AppContext as _;
        cx.update_window(w, |_, window, cx| window.simulate_next_frame(cx)).expect("the window");
    };
    let at = |cx: &mut TestAppContext| {
        view.read_with(cx, |v, _| {
            (v.model.lives[live].board.steps.load(std::sync::atomic::Ordering::Relaxed), v.model.lives[live].board.published)
        })
    };
    let drawn = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.model.lives[live].transcript.drawn().cloned().expect("a board"));
    let before = drawn(cx);
    // A long history: its events past many slices.
    let (seq, sid) = super::answer_tests::attachment(cx, &view);
    view.update(cx, |v, cx| {
        let base_len = v.model.lives[live].transcript.history_len();
        let messages = synaps_client::synthetic::history(30_000, 3);
        let update = synaps_client::HistoryUpdate::Append { base_len, messages };
        v.apply_batch(vec![synaps_client::Event::History { attach_seq: seq, session_id: sid.clone(), seq: 0, update }], cx);
    });
    frame(cx);
    let (s0, p0) = at(cx);
    let mut ran = 0;
    while at(cx).0 < s0 + 3 && at(cx).1 == p0 && ran < 10_000 {
        cx.executor().tick();
        ran += 1;
    }
    let (s1, p1) = at(cx);
    assert_eq!(p1, p0, "paused mid-fold: nothing published");
    assert!(s1 >= s0 + 3, "the job ran three steps ({s0} → {s1})");
    frame(cx);
    assert!(std::sync::Arc::ptr_eq(&drawn(cx), &before), "the frames draw the board before it");
    // A live round mid-fold: another input.
    let evs = vec![
        json!({"ev": "turn_started", "turn_baseline": 13, "trigger": "user", "user_text": "go on"}),
        llm(json!({"kind": "llm", "llm": "response_start"})),
        llm(json!({"kind": "llm", "llm": "tool_use_start", "tool_name": "cards_block", "tool_id": "toolu_snap"})),
        llm(
            json!({"kind": "llm", "llm": "tool_use", "tool_name": "cards_block", "tool_id": "toolu_snap", "input": block_input(json!([{"op":"put","id":"snap","kind":"note","body":{"text":"the snapshot"}}]))}),
        ),
    ];
    view.update(cx, |v, cx| {
        let batch = evs
            .into_iter()
            .map(|e| {
                let ev = synaps_client::protocol::decode_event(&e).unwrap();
                synaps_client::Event::session(seq, sid.clone(), 0, ev, std::sync::Arc::new(e))
            })
            .collect();
        v.apply_batch(batch, cx);
    });
    frame(cx);
    assert_eq!(at(cx).1, p0, "still nothing published");
    cx.run_until_parked();
    settle(cx, w, 8);
    let (_, p2) = at(cx);
    assert_eq!(p2, p0 + 1, "one board published, once");
    let d = drawn(cx);
    let (epoch, history, events) = view.read_with(cx, |v, _| {
        let i = v.model.lives[live].transcript.blocks_input().expect("blocks");
        (i.epoch, i.history_len, i.history_len + i.live.events.len())
    });
    assert_eq!((d.epoch, d.history, d.events), (epoch, history, events), "the input as it is now, whole");
    assert!(row(cx, &view, "snap").is_some(), "the live round's block drawn");
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
        (crate::board::events(&i), i.generation)
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

/// A re-fold holds for the input it read, not just its history's epoch
/// (Shady #101 P2-2). An outcome, which reads the whole input, done: the
/// same request after a live result changed the input runs again, and
/// finds the new input's board. Running when an append changes it (the
/// same epoch): stopped before its first turn, and run again on the new
/// input, never finished on the old. A revision whose batch committed in
/// the history reads that history only: a live tick changes nothing of it,
/// and it is kept.
#[gpui_kit::test]
fn a_refold_holds_for_the_input_it_read(cx: &mut TestAppContext) {
    use cards_spec::board::{Pos, Refold, Target};
    let (w, view) = blocks(cx, 1500.);
    let live = view.read_with(cx, |v, _| v.panes.cur.live);
    let reference = |cx: &mut TestAppContext, at: Target| {
        view.read_with(cx, |v, _| {
            let i = v.model.lives[live].transcript.blocks_input().expect("blocks");
            let events = crate::board::events(&i);
            let mut rf = Refold::to(i.generation, &events, at).unwrap();
            while !rf.step(usize::MAX).unwrap() {}
            rf.state().cloned()
        })
    };
    let done = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.refolds.done.clone());
    let started = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.refolds.started);
    let state = |cx: &mut TestAppContext| done(cx).and_then(|d| d.state.ok().flatten()).map(|s| (*s).clone());
    let outcome = Target::Outcome(Pos::event(0));
    view.update(cx, |v, cx| v.refold(live, outcome, cx));
    cx.run_until_parked();
    let before = state(cx).expect("an outcome");
    assert_eq!(Some(before.clone()), reference(cx, outcome));
    // A live round patches a block: the same epoch, another input.
    let epoch = view.read_with(cx, |v, _| v.model.lives[live].transcript.blocks_epoch());
    turn(cx, w, &view);
    round(cx, w, &view, &[("toolu_p2", json!([{"op":"patch","id":"now","set":{"detail":"after the outcome"}}]), true)]);
    assert_eq!(view.read_with(cx, |v, _| v.model.lives[live].transcript.blocks_epoch()), epoch, "the same history");
    assert!(done(cx).is_none(), "an outcome over another input is dropped");
    view.update(cx, |v, cx| v.refold(live, outcome, cx));
    cx.run_until_parked();
    assert_eq!(started(cx), 2, "the same request runs again");
    let after = state(cx).expect("an outcome");
    assert_ne!(after, before);
    assert_eq!(Some(after), reference(cx, outcome), "the new input's");
    // Running when the input moves on: never finished on the old one.
    view.update(cx, |v, cx| {
        v.refold(live, outcome, cx);
        v.refold_cancel();
        v.refold(live, outcome, cx);
    });
    let n = started(cx);
    view.update(cx, |v, cx| {
        let t = &mut v.model.lives[live].transcript;
        t.apply(
            &synaps_client::protocol::decode_event(
                &json!({"ev": "turn_started", "turn_baseline": 14, "trigger": "user", "user_text": "more"}),
            )
            .unwrap(),
        );
        v.sync_board(live, cx);
    });
    cx.run_until_parked();
    settle(cx, w, 8);
    assert_eq!(started(cx), n + 1, "run again on the new input");
    assert_eq!(state(cx), reference(cx, outcome), "the new input's, not the old one's");
    // A revision in the history: a live tick doesn't touch it.
    let at = Target::Revision(Pos::event(8));
    view.update(cx, |v, cx| v.refold(live, at, cx));
    cx.run_until_parked();
    let n = started(cx);
    let kept = state(cx);
    assert_eq!(kept, reference(cx, at));
    round(cx, w, &view, &[("toolu_p3", json!([{"op":"patch","id":"now","set":{"detail":"a tick"}}]), true)]);
    view.update(cx, |v, cx| v.refold(live, at, cx));
    cx.run_until_parked();
    assert_eq!((started(cx), state(cx)), (n, kept), "kept: nothing ran");
}

/// An edit preview is built as its file's calls commit, never as rows
/// draw (Shady #101 P2-3): a history of 40 more edits of the file keeps
/// its groups bounded, the newest kept and the rest counted; text
/// streaming in (the rows change, the files don't) builds none while the
/// preview draws open, and the draws take the one built; the next edit of
/// the file landing builds it once, its group the newest.
#[gpui_kit::test]
fn an_edit_preview_is_built_as_edits_commit_not_as_rows_draw(cx: &mut TestAppContext) {
    let (w, view) = blocks(cx, 1500.);
    let file = "crates/relay/src/token_store.rs";
    let edit = |k: usize| {
        let id = format!("toolu_pe{k}");
        vec![
            json!({"role": "assistant", "content": [{"type": "tool_use", "id": id, "name": "edit", "input": {"path": file, "old_string": format!("a{k}"), "new_string": format!("b{k}")}}]}),
            json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": id, "content": format!("Edited {file}")}]}),
        ]
    };
    let mut msgs = vec![json!({"role": "user", "content": "more edits"})];
    msgs.extend((0..40).flat_map(edit));
    land(cx, w, &view, msgs);
    board(cx, w, &view);
    // The agent folded its edits: open them (once nothing just moved).
    let edits = show_block(cx, w, &view, "edits");
    wait(cx, w, &view, 1000);
    let at = head_at(cx, w, edits);
    mouse(cx, w, at, "move");
    mouse(cx, w, at, "down");
    mouse(cx, w, at, "up");
    board(cx, w, &view);
    assert!(!folded(cx, &view, edits), "open");
    let built = || crate::files::EDIT_PREVIEWS.with(std::cell::Cell::get);
    let preview = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].shown_files().edit_preview(file));
    let before = preview(cx).expect("the file's");
    assert!(before.groups.len() <= 12 && before.earlier >= 41 - 12, "{} groups, {} earlier", before.groups.len(), before.earlier);
    let n = built();
    // Text streams: rows change, frames draw, the files stay.
    turn(cx, w, &view);
    heard(cx, w, &view, vec![llm(json!({"kind": "llm", "llm": "response_start"}))]);
    for k in 0..20 {
        heard(cx, w, &view, vec![llm(json!({"kind": "llm", "llm": "text", "text": format!("streaming {k} ")}))]);
        board(cx, w, &view);
    }
    assert!(!folded(cx, &view, edits) && row(cx, &view, "edits").is_some(), "still drawn open");
    assert_eq!(built(), n, "no preview built while text streams");
    assert!(std::sync::Arc::ptr_eq(&before, &preview(cx).unwrap()), "the one built");
    heard(cx, w, &view, vec![json!({"ev": "idle"})]);
    land(cx, w, &view, edit(99));
    board(cx, w, &view);
    assert!(built() > n, "the next edit builds it");
    let after = preview(cx).unwrap();
    let newest = after.groups.last().unwrap().lines.iter().any(|l| l.text.contains("b99"));
    assert!(newest, "the newest group is the new edit's");
}

/// Session `sid` in a new tab of the pane acted on, shown.
fn new_tab(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, sid: &str) {
    let client = view.update(cx, |v, cx| {
        let c = v.client.take();
        v.open_tab(crate::app::Open::Attach { session_id: sid.into(), fallback: None, open: false }, crate::tabs::Place::New);
        cx.notify();
        c
    });
    cx.run_until_parked();
    settle(cx, w, 8);
    view.update(cx, |v, _| v.client = client);
    board(cx, w, view);
}

/// Tab `ix` shown.
fn show_tab(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, ix: usize) {
    let client = view.update(cx, |v, cx| {
        let c = v.client.take();
        v.focus_tab(ix);
        cx.notify();
        c
    });
    cx.run_until_parked();
    settle(cx, w, 8);
    view.update(cx, |v, _| v.client = client);
    board(cx, w, view);
}

/// The pane acted on's current tab closed.
fn close_current(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    view.update(cx, |v, cx| {
        let ix = v.model.tabs.active().expect("a tab");
        v.close_tab(ix);
        cx.notify();
    });
    cx.run_until_parked();
    wait(cx, w, view, 1000);
    board(cx, w, view);
}

/// A history round putting `ops` (call `tool`), landed.
fn land_round(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, tool: &str, ops: Value) {
    land(
        cx,
        w,
        view,
        vec![
            json!({"role": "user", "content": "go on"}),
            json!({"role": "assistant", "content": [{"type": "tool_use", "id": tool, "name": "cards_block", "input": block_input(ops.clone())}]}),
            json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": tool, "content": block_receipt(&ops)}]}),
        ],
    );
    board(cx, w, view);
}

/// The pane acted on's block state, and its store's board work: folds,
/// notes opened, wells' scroll, painted targets, then a re-fold kept for
/// the store, a board waiting, a job running.
fn block_state(cx: &mut TestAppContext, view: &Entity<AppView>) -> (usize, usize, usize, usize, bool, bool, bool) {
    view.read_with(cx, |v, _| {
        let (p, live) = (&v.panes.cur, v.panes.cur.live);
        let b = &v.model.lives[live].board;
        let refold = v.refolds.done.as_ref().is_some_and(|d| d.live == live);
        let state = (p.block_folds.rows().len(), p.block_more.len(), p.block_scroll.borrow().len(), p.lease.painted.borrow().len());
        (state.0, state.1, state.2, state.3, refold, b.waiting.is_some(), b.running())
    })
}

/// A session's block state and its board's work go where its transcript
/// goes (Shady #101 P2-4). The user folds A's plan, and a re-fold of A is
/// kept. A switch to B in another tab of the same pane and store leaves
/// nothing of A's on the pane or the store (no fold, opened note, scroll,
/// painted target, re-fold, waiting board or job), and B, with no blocks,
/// keeps no fold. B's own plan (the same name: the same row) isn't drawn
/// folded by A's fold. Back on A, its fold comes back with its tab, and
/// its board reads on where it was: a new op draws. A session with no
/// blocks, then the last tab closed: nothing kept on the pane or store.
#[gpui_kit::test]
fn a_sessions_block_state_and_board_work_go_with_its_transcript(cx: &mut TestAppContext) {
    use cards_spec::board::{Pos, Target};
    let (w, view) = blocks(cx, 1500.);
    let live = view.read_with(cx, |v, _| v.panes.cur.live);
    let plan = show_block(cx, w, &view, "plan");
    wait(cx, w, &view, 1000);
    let at = head_at(cx, w, plan);
    mouse(cx, w, at, "move");
    mouse(cx, w, at, "down");
    mouse(cx, w, at, "up");
    board(cx, w, &view);
    assert!(folded(cx, &view, plan), "A's plan, folded by the user");
    view.update(cx, |v, cx| v.refold(live, Target::Outcome(Pos::event(0)), cx));
    cx.run_until_parked();
    let a = block_state(cx, &view);
    assert!(a.0 == 1 && a.3 > 0 && a.4, "A's: {a:?}");
    let fold = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.model.lives[live].transcript.blocks_retained().journal);
    assert!(fold(cx) > 0, "A's fold");
    // B, in a new tab: nothing of A's.
    new_tab(cx, w, &view, crate::demo::SID);
    assert_eq!(view.read_with(cx, |v, _| v.panes.cur.live), live, "the same store");
    assert_eq!(block_state(cx, &view), (0, 0, 0, 0, false, false, false), "nothing of A's");
    assert_eq!(fold(cx), 0, "B has no blocks: no fold kept");
    land_round(
        cx,
        w,
        &view,
        "toolu_b1",
        json!([{"op":"put","id":"plan","kind":"plan","body":{"steps":[{"id":"a","label":"A","state":"doing"}]}}]),
    );
    let (plan_b, _) = row(cx, &view, "plan").expect("B's plan drawn");
    assert_eq!(plan_b, plan, "the same row id");
    assert!(!folded(cx, &view, plan_b), "B's plan isn't folded by A's fold");
    // Back on A: its fold, and its board reads on.
    show_tab(cx, w, &view, 0);
    assert!(folded(cx, &view, plan), "A's fold came back with its tab");
    turn(cx, w, &view);
    round(cx, w, &view, &[("toolu_a9", json!([{"op":"put","id":"back","kind":"note","body":{"text":"back on A"}}]), true)]);
    board(cx, w, &view);
    assert!(row(cx, &view, "back").is_some(), "A's board reads on: the new op draws");
    heard(cx, w, &view, vec![json!({"ev": "idle"})]);
    // A session with no blocks, then the last tab closed.
    new_tab(cx, w, &view, crate::demo::IDLE_CPU);
    assert_eq!(block_state(cx, &view), (0, 0, 0, 0, false, false, false), "nothing of A's");
    assert_eq!(fold(cx), 0, "no blocks: no fold kept");
    while view.read_with(cx, |v, _| !v.model.tabs.is_empty()) {
        close_current(cx, w, &view);
    }
    assert_eq!(block_state(cx, &view), (0, 0, 0, 0, false, false, false), "the last tab closed: nothing kept");
    assert_eq!(fold(cx), 0);
}

/// Split panes (Shady #101 P2-4): a pane sharing a store with another, and
/// closing, leaves the store's board work to the pane that stays (it reads
/// on: a new op draws there), and its closing rows fold nothing; a pane on
/// a store of its own, closing, lets go of that store's work and its
/// re-folds.
#[gpui_kit::test]
fn a_closing_split_pane_leaves_a_shared_boards_work_and_lets_go_of_its_own(cx: &mut TestAppContext) {
    use cards_spec::board::{Pos, Target};
    let (w, view) = blocks(cx, 1500.);
    view.update(cx, |v, _| v.app_settings.split = true);
    let (first, live) = view.read_with(cx, |v, _| (v.panes.cur.id, v.panes.cur.live));
    let split = |cx: &mut TestAppContext| {
        use gpui_kit::AppContext as _;
        cx.update_window(w, |_, window, cx| view.update(cx, |v, cx| v.split_pane(None, window, cx))).expect("the window");
        cx.run_until_parked();
        settle(cx, w, 8);
        board(cx, w, &view);
    };
    // Shared: the new pane on A's store, closing.
    split(cx);
    let (second, shared) = view.read_with(cx, |v, _| (v.panes.cur.id, v.panes.cur.live));
    assert!(second != first && shared == live, "the second pane shares A's store");
    view.update(cx, |v, cx| {
        let ix = v.model.tabs.active().expect("a tab");
        v.close_tab(ix);
        cx.notify();
    });
    cx.run_until_parked();
    let closing = view.read_with(cx, |v, _| v.panes.iter().find(|p| p.id == second).map(|p| p.live));
    if let Some(c) = closing.filter(|c| *c != live) {
        settle(cx, w, 8);
        assert!(!view.read_with(cx, |v, _| v.model.lives.get(c).is_some_and(|l| l.board.running())), "its closing rows fold nothing");
    }
    wait(cx, w, &view, 1000);
    board(cx, w, &view);
    assert_eq!(view.read_with(cx, |v, _| (v.panes.len(), v.panes.cur.id)), (1, first), "closed");
    turn(cx, w, &view);
    round(cx, w, &view, &[("toolu_s1", json!([{"op":"put","id":"stays","kind":"note","body":{"text":"the pane that stays"}}]), true)]);
    board(cx, w, &view);
    assert!(row(cx, &view, "stays").is_some(), "the shared store's board reads on");
    heard(cx, w, &view, vec![json!({"ev": "idle"})]);
    // Its own: B in a new pane, a re-fold of it kept, the pane closing.
    split(cx);
    new_tab(cx, w, &view, crate::demo::SID);
    view.update(cx, |v, cx| {
        let ix = v.model.tabs.iter().position(|t| t.session.as_deref() == Some(crate::demo::BLOCKS)).expect("A's tab");
        v.close_tab(ix);
        cx.notify();
    });
    cx.run_until_parked();
    board(cx, w, &view);
    let own = view.read_with(cx, |v, _| v.panes.cur.live);
    assert_ne!(own, live, "B on a store of its own");
    land_round(
        cx,
        w,
        &view,
        "toolu_b1",
        json!([{"op":"put","id":"plan","kind":"plan","body":{"steps":[{"id":"a","label":"A","state":"doing"}]}}]),
    );
    assert!(row(cx, &view, "plan").is_some(), "B's plan drawn");
    view.update(cx, |v, cx| v.refold(own, Target::Outcome(Pos::event(0)), cx));
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| v.refolds.done.as_ref().is_some_and(|d| d.live == own)), "B's re-fold kept");
    close_current(cx, w, &view);
    view.read_with(cx, |v, _| {
        assert_eq!((v.panes.len(), v.panes.cur.id), (1, first), "closed");
        assert!(!v.refolds.done.as_ref().is_some_and(|d| d.live == own), "B's re-fold let go");
        assert!(
            v.model.lives.get(own).is_none_or(|l| l.transcript.blocks_retained().journal == 0 && !l.board.running()),
            "B's store let go"
        );
    });
    assert!(row(cx, &view, "stays").is_some(), "A's rows, as they were");
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
