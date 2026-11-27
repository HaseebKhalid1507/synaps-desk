//! D6a's rail in the window (synaps-cards `docs/blocks.md` §4, the D6 row
//! of §8.2): both slots around Files, the cap across them as clicked, a
//! header clicked to expand keeping its y, the rail's lease and painted
//! targets, the strip's `◆N`, split panes (the panel follows the focused
//! pane), the fit to the panel's 296 px floor and wide, NOW in the window,
//! sections coming and going on the glide, and nothing drawn at rest. The
//! layout's own tests are `agent_rail/tests.rs`.
//!
//! The helpers are `ui_tests`' own, from [`super`] (`test_kit`), and D5's
//! window helpers (`blocks_ui_tests`).

use gpui_kit::{AnyWindowHandle, Bounds, Entity, Pixels, Point, TestAppContext, point, px};
use serde_json::{Value, json};
use synaps_client::blocks::Ref;

use super::answer_tests::{has, heard, llm};
use super::blocks_ui_tests::{board, bounds, click, drawn, mouse, open_session, round};
use super::{open_at, settle, wait};
use crate::app::AppView;
use crate::tabs::Place;

type W = (AnyWindowHandle, Entity<AppView>);

/// Tall enough for the whole panel unscrolled.
const TALL: f32 = 2400.;

/// The rail demo session (`synthetic::rail_history`) in a window `w` px
/// wide, its board drawn.
fn rail_at(cx: &mut TestAppContext, w: f32, h: f32) -> W {
    let (win, view, _events) = open_at(cx, (w, h), crate::theme::Panels::default().text_size);
    open_session(cx, win, &view, crate::demo::RAIL, Place::Current);
    (win, view)
}

fn rail(cx: &mut TestAppContext) -> W {
    rail_at(cx, 1280., TALL)
}

fn row(name: &str) -> u64 {
    crate::blocks::row_id(&Ref::parse(name).expect("an id"))
}

fn sec(cx: &mut TestAppContext, w: AnyWindowHandle, name: &str) -> Bounds<Pixels> {
    drawn(cx, w, format!("agent-section {}", row(name)))
}

/// Section `name`'s height as drawn: its glide's, as it comes and goes.
fn glide(cx: &mut TestAppContext, w: AnyWindowHandle, name: &str) -> f32 {
    f32::from(drawn(cx, w, format!("agent-glide {}", row(name))).size.height)
}

fn has_sec(cx: &mut TestAppContext, w: AnyWindowHandle, name: &str) -> bool {
    has(cx, w, format!("agent-section {}", row(name)))
}

fn head(cx: &mut TestAppContext, w: AnyWindowHandle, name: &str) -> Point<Pixels> {
    drawn(cx, w, format!("agent-head {}", row(name))).center()
}

fn open(cx: &mut TestAppContext, w: AnyWindowHandle, name: &str) -> bool {
    has(cx, w, format!("agent-body {}", row(name)))
}

/// The slot's `+N` row drawn, its text.
fn more(cx: &mut TestAppContext, view: &Entity<AppView>, k: usize) -> Option<String> {
    view.read_with(cx, |v, _| v.rail_layout().and_then(|(_, l)| l.more[k].as_ref().map(crate::agent_rail::more_text)))
}

/// A turn on the rail session (its history is 6 messages).
fn go(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    heard(cx, w, view, vec![json!({"ev": "turn_started", "turn_baseline": 6, "trigger": "user", "user_text": "go on"})]);
}

fn ops(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, tool: &str, ops: Value) {
    round(cx, w, view, &[(tool, ops, true)]);
}

fn waiting(cx: &mut TestAppContext, view: &Entity<AppView>) -> bool {
    view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].board.waiting.is_some())
}

fn says(cx: &mut TestAppContext, view: &Entity<AppView>) -> Option<(u64, &'static str)> {
    view.read_with(cx, |v, _| v.agent_rail.lease.note)
}

/// The pointer off the panel (over the transcript): the rail's lease ends.
fn away(cx: &mut TestAppContext, w: AnyWindowHandle) {
    mouse(cx, w, point(px(700.), px(400.)), "move");
}

#[gpui_kit::test]
fn the_upper_slot_draws_before_files_and_the_end_slot_after(cx: &mut TestAppContext) {
    let (w, _view) = rail(cx);
    let upper = drawn(cx, w, "agent-zone 0".into());
    let files = drawn(cx, w, "right-files".into());
    let end = drawn(cx, w, "agent-zone 1".into());
    assert!(upper.bottom() <= files.top() + px(0.5) && files.bottom() <= end.top() + px(0.5), "{upper:?} {files:?} {end:?}");
    if let Some(cache) = bounds(cx, w, "right-cache".into()) {
        assert!(cache.bottom() <= upper.top() + px(0.5), "Prompt cache, then the agent's");
    }
    assert!(has(cx, w, "agent-count 7 sections".into()));
    assert!(has(cx, w, "agent-divider".into()), "the end slot's thin AGENT divider");
    let tops: Vec<f32> = ["plan", "now", "cause", "file"].iter().map(|n| f32::from(sec(cx, w, n).top())).collect();
    assert!(tops.windows(2).all(|t| t[0] < t[1]), "placement order: {tops:?}");
    for n in ["plan", "now", "cause", "file"] {
        let s = sec(cx, w, n);
        assert!(upper.contains(&s.origin) && open(cx, w, n), "{n} open in the upper slot");
    }
    // The end slot: the group folded (no place), then `+2 more`.
    let g = sec(cx, w, "findings");
    assert!(end.contains(&g.origin) && !open(cx, w, "findings"));
    assert!(has(cx, w, format!("agent-summary {} 2 blocks", row("findings"))));
    let m = drawn(cx, w, "agent-more 1 +2 more · rot, retry".into());
    assert!(g.bottom() <= m.top() + px(0.5));
    assert!(!has_sec(cx, w, "rot") && !has_sec(cx, w, "retry"), "past the cap: in the +N row");
    // Every section is the agent's frame: its spine and its chip.
    for n in ["plan", "now", "cause", "file", "findings"] {
        assert!(has(cx, w, format!("agent-spine {}", row(n))) && has(cx, w, format!("agent-chip {}", row(n))), "{n}");
    }
}

#[gpui_kit::test]
fn the_cap_holds_across_slots_as_clicked(cx: &mut TestAppContext) {
    let (w, view) = rail(cx);
    let m = drawn(cx, w, "agent-more 1 +2 more · rot, retry".into()).center();
    click(cx, w, m);
    wait(cx, w, &view, 1000);
    assert!(has_sec(cx, w, "rot") && has_sec(cx, w, "retry"), "their headers listed");
    assert!(!open(cx, w, "rot") && !open(cx, w, "retry"));
    // The user's expansion takes a place first: the agent's last in
    // drawing order (file, in the other slot) gives way.
    let at = head(cx, w, "rot");
    click(cx, w, at);
    wait(cx, w, &view, 1000);
    assert!(open(cx, w, "rot"), "rot expanded");
    assert!(!has_sec(cx, w, "file"), "file gave its place");
    assert_eq!(more(cx, &view, 0).as_deref(), Some("+1 more · file"));
    assert!(has(cx, w, "agent-more 0 +1 more · file".into()));
    // A collapse frees a place: file has it back, as its agent made it.
    let at = head(cx, w, "plan");
    click(cx, w, at);
    wait(cx, w, &view, 1000);
    assert!(has(cx, w, format!("agent-summary {} 6/20 done", row("plan"))), "plan folded to its summary");
    assert!(open(cx, w, "file") && more(cx, &view, 0).is_none(), "file back");
    assert_eq!(settle(cx, w, 8), 0, "at rest");
}

#[gpui_kit::test]
fn a_header_clicked_to_expand_keeps_its_y(cx: &mut TestAppContext) {
    // A short window: the panel scrolls, at its end.
    let (w, view) = rail_at(cx, 1280., 900.);
    let end = |cx: &mut TestAppContext| {
        view.update(cx, |v, cx| {
            v.bars.right.scroll.set_offset(point(px(0.), px(-100_000.)));
            cx.notify();
        });
        settle(cx, w, 8);
    };
    end(cx);
    let m = drawn(cx, w, "agent-more 1 +2 more · rot, retry".into()).center();
    click(cx, w, m);
    wait(cx, w, &view, 1000);
    end(cx);
    let y0 = drawn(cx, w, format!("agent-head {}", row("retry"))).top();
    let at = head(cx, w, "retry");
    click(cx, w, at);
    // File folds away above it as retry opens: retry's header stays put,
    // frame by frame, the panel scrolling instead.
    let mut ys = Vec::new();
    for _ in 0..12 {
        wait(cx, w, &view, 40);
        ys.push(f32::from(drawn(cx, w, format!("agent-head {}", row("retry"))).top()));
    }
    wait(cx, w, &view, 1000);
    ys.push(f32::from(drawn(cx, w, format!("agent-head {}", row("retry"))).top()));
    assert!(open(cx, w, "retry") && !has_sec(cx, w, "file"), "retry open, file past the cap");
    assert!(ys.iter().all(|y| (y - f32::from(y0)).abs() <= 1.), "retry's header kept {y0:?}: {ys:?}");
}

#[gpui_kit::test]
fn a_change_to_the_rails_structure_waits_under_the_pointer(cx: &mut TestAppContext) {
    let (w, view) = rail(cx);
    go(cx, w, &view);
    // Over the rail: a section added above the pointer waits, the whole
    // board with it.
    let at = head(cx, w, "cause");
    mouse(cx, w, at, "move");
    let new = json!([{"op": "put", "id": "extra", "kind": "note", "title": "Extra", "target": "rail", "slot": "upper", "at": {"after": "plan"}, "body": {"text": "one more"}}]);
    ops(cx, w, &view, "toolu_x1", new);
    assert!(waiting(cx, &view), "the board waits under the rail's lease");
    assert!(!has_sec(cx, w, "extra"));
    // A change that keeps the rail's structure lands: a status's text, its
    // two lines the same.
    away(cx, w);
    wait(cx, w, &view, 1000);
    assert!(!waiting(cx, &view) && has_sec(cx, w, "extra"), "the lease over: it lands");
    let at = head(cx, w, "cause");
    mouse(cx, w, at, "move");
    let same = json!([{"op": "patch", "id": "now", "set": {"text": "Running it again"}}]);
    ops(cx, w, &view, "toolu_x2", same);
    assert!(!waiting(cx, &view), "a change in place lands under the lease");
    let text = view.read_with(cx, |v, _| {
        v.rail_layout().and_then(|(_, l)| l.section(&Ref::parse("now").expect("an id")).map(|s| crate::agent_rail::summary(&s.s)))
    });
    assert_eq!(text.as_deref(), Some("Running it again"));
}

#[gpui_kit::test]
fn a_press_on_a_section_replaced_under_it_does_nothing(cx: &mut TestAppContext) {
    let (w, view) = rail(cx);
    go(cx, w, &view);
    let at = head(cx, w, "cause");
    mouse(cx, w, at, "move");
    mouse(cx, w, at, "down");
    assert!(view.read_with(cx, |v, _| v.agent_rail.lease.pressed), "the press holds the lease");
    // The agent replaces `cause`: another incarnation, the same lines.
    let again =
        json!([{"op": "replace", "id": "cause", "kind": "note", "title": "Root cause", "body": {"text": "Another cause.\n".repeat(20)}}]);
    ops(cx, w, &view, "toolu_r1", again);
    mouse(cx, w, at, "up");
    assert!(open(cx, w, "cause"), "nothing folded");
    assert_eq!(says(cx, &view), Some((row("cause"), crate::lease::CHANGED)));
    assert!(has(cx, w, format!("agent-says {} {}", row("cause"), crate::lease::CHANGED)));
    // Its `+N` row's target too: its leftovers changed under the press.
    let m = drawn(cx, w, "agent-more 1 +2 more · rot, retry".into()).center();
    mouse(cx, w, m, "move");
    mouse(cx, w, m, "down");
    let gone = json!([{"op": "remove", "id": "retry"}]);
    ops(cx, w, &view, "toolu_r2", gone);
    mouse(cx, w, m, "up");
    assert!(view.read_with(cx, |v, _| !v.agent_rail.local(crate::demo::RAIL).listed[1]), "not listed");
    assert_eq!(says(cx, &view).map(|s| s.1), Some(crate::lease::CHANGED));
    // Once the lease ends, the board lands; a fresh click acts.
    away(cx, w);
    wait(cx, w, &view, 1000);
    assert!(!waiting(cx, &view));
    let at = head(cx, w, "cause");
    click(cx, w, at);
    wait(cx, w, &view, 1000);
    assert!(!open(cx, w, "cause"), "folded");
}

#[gpui_kit::test]
fn a_target_an_agent_op_just_moved_does_nothing_once(cx: &mut TestAppContext) {
    let (w, view) = rail(cx);
    go(cx, w, &view);
    away(cx, w);
    // The agent moves plan after cause.
    let mv = json!([{"op": "move", "id": "plan", "slot": "upper", "at": {"after": "cause"}}]);
    ops(cx, w, &view, "toolu_m1", mv);
    assert!(sec(cx, w, "cause").top() < sec(cx, w, "plan").top(), "plan after cause");
    let at = head(cx, w, "plan");
    click(cx, w, at);
    assert!(open(cx, w, "plan"), "just moved: nothing");
    assert_eq!(says(cx, &view), Some((row("plan"), crate::lease::MOVED)));
    let at = head(cx, w, "plan");
    click(cx, w, at);
    wait(cx, w, &view, 1000);
    assert!(!open(cx, w, "plan"), "the next click folds it");
}

#[gpui_kit::test]
fn the_strip_counts_the_sections_and_opens_the_panel(cx: &mut TestAppContext) {
    let (w, view) = rail(cx);
    view.update(cx, |v, cx| {
        let p = crate::theme::Panels { right_collapsed: true, ..v.panels };
        v.set_panels_pub(p, cx);
    });
    wait(cx, w, &view, 1000);
    assert!(!has(cx, w, "agent-zone 0".into()), "the panel folded");
    let n = drawn(cx, w, "strip-agent 7".into());
    click(cx, w, n.center());
    wait(cx, w, &view, 1500);
    assert!(view.read_with(cx, |v, _| !v.panels.right_collapsed), "the strip's click opened the panel");
    assert!(has(cx, w, "agent-zone 0".into()) && !has(cx, w, "strip-agent 7".into()));
    // A session with none: no count.
    view.update(cx, |v, cx| {
        let p = crate::theme::Panels { right_collapsed: true, ..v.panels };
        v.set_panels_pub(p, cx);
    });
    open_session(cx, w, &view, crate::demo::SID, Place::Current);
    wait(cx, w, &view, 1000);
    assert!(!(0..10).any(|n| has(cx, w, format!("strip-agent {n}"))), "no ◆ without sections");
}

#[gpui_kit::test]
fn with_split_panes_the_panel_follows_the_focused_pane(cx: &mut TestAppContext) {
    let (w, view) = rail_at(cx, 1800., TALL);
    super::blocks_ui_tests::split(cx, w, &view);
    let panes = view.read_with(cx, |v, _| v.panes.ids());
    view.update(cx, |v, cx| v.focus_pane(panes[0], crate::pane::KeysTo::Pane, cx));
    open_session(cx, w, &view, crate::demo::SID, Place::Current);
    wait(cx, w, &view, 1000);
    assert!(!has(cx, w, "agent-zone 0".into()), "pane 1's session has no sections");
    view.update(cx, |v, cx| v.focus_pane(panes[1], crate::pane::KeysTo::Pane, cx));
    board(cx, w, &view);
    wait(cx, w, &view, 1000);
    assert!(has(cx, w, "agent-count 7 sections".into()), "pane 2's rail");
    // Its local state is its session's: a fold made here is there when
    // the other pane focuses the same session.
    let at = head(cx, w, "plan");
    click(cx, w, at);
    wait(cx, w, &view, 1000);
    assert!(!open(cx, w, "plan"));
    view.update(cx, |v, cx| v.focus_pane(panes[0], crate::pane::KeysTo::Pane, cx));
    open_session(cx, w, &view, crate::demo::RAIL, Place::Current);
    wait(cx, w, &view, 1000);
    assert!(has(cx, w, "agent-count 7 sections".into()) && !open(cx, w, "plan"), "the session's fold, in the other pane");
}

/// Each section inside the panel's content width, and no taller than its
/// kind's 12 lines (its header, 20 px a line, its padding).
fn fits(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, width: f32) {
    let zone = drawn(cx, w, "agent-zone 0".into());
    assert!((f32::from(zone.size.width) - width).abs() <= 1., "the zone is the panel's width: {zone:?} at {width}");
    let names = view.read_with(cx, |v, _| {
        v.rail_layout().map(|(_, l)| l.sections().map(|s| s.s.block.id.to_string()).collect::<Vec<_>>()).unwrap_or_default()
    });
    for n in &names {
        if !has_sec(cx, w, n) {
            continue;
        }
        let s = sec(cx, w, n);
        assert!(s.left() >= zone.left() + px(23.) && s.right() <= zone.right() - px(23.), "{n} inside the panel's insets at {width}");
        assert!(f32::from(s.size.height) <= 28. + 12. * 20. + 6. + 0.5, "{n} keeps to 12 lines at {width}: {s:?}");
        let h = drawn(cx, w, format!("agent-head {}", row(n)));
        assert!(h.right() <= s.right() + px(0.5), "{n}'s header fits");
    }
}

#[gpui_kit::test]
fn sections_fit_the_panels_floor_and_a_wide_panel(cx: &mut TestAppContext) {
    let (w, view) = rail_at(cx, 1600., TALL);
    // The floor: 296 px (blocks.md §4: the doc's 320, the panel's floor).
    let floor = crate::app::FLOORS.1;
    let m = drawn(cx, w, "agent-more 1 +2 more · rot, retry".into()).center();
    click(cx, w, m);
    wait(cx, w, &view, 1000);
    for n in ["rot", "retry"] {
        let at = head(cx, w, n);
        click(cx, w, at);
        wait(cx, w, &view, 1000);
    }
    fits(cx, w, &view, floor);
    // The diff at the floor: 10 lines, `+4 lines` and its foot.
    assert!(has(cx, w, format!("agent-diff-more {} +4 lines", row("retry"))), "the diff's overflow");
    // The plan: its window and `+9 more · 4 done`.
    assert!(has(cx, w, format!("agent-plan-more {} +9 more · 4 done", row("plan"))), "the plan's overflow");
    // Wide: 520 px.
    view.update(cx, |v, cx| {
        let p = crate::theme::Panels { right_width: Some(520), ..v.panels };
        v.set_panels_pub(p, cx);
    });
    wait(cx, w, &view, 1500);
    let wide = f32::from(drawn(cx, w, "agent-zone 0".into()).size.width);
    assert!((wide - 520.).abs() <= 1., "wide: {wide}");
    fits(cx, w, &view, wide);
}

/// Frames the window asks for at rest, with the panel open and folded.
fn rest(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) -> (usize, usize) {
    let fold = |cx: &mut TestAppContext, folded: bool| {
        view.update(cx, |v, cx| {
            let p = crate::theme::Panels { right_collapsed: folded, ..v.panels };
            v.set_panels_pub(p, cx);
        });
        wait(cx, w, view, 2000);
        settle(cx, w, 8)
    };
    let folded = fold(cx, true);
    (fold(cx, false), folded)
}

#[gpui_kit::test]
fn now_runs_on_the_beat_only_while_the_turn_streams(cx: &mut TestAppContext) {
    let (w, view) = rail(cx);
    assert!(has(cx, w, "agent-now idle".into()) && has(cx, w, "agent-now-life Done".into()));
    assert_eq!(settle(cx, w, 8), 0, "idle: no frames");
    go(cx, w, &view);
    heard(
        cx,
        w,
        &view,
        vec![
            llm(json!({"kind": "llm", "llm": "response_start"})),
            llm(json!({"kind": "llm", "llm": "tool_use_start", "tool_name": "read", "tool_id": "toolu_n1"})),
            llm(json!({"kind": "llm", "llm": "tool_use", "tool_name": "read", "tool_id": "toolu_n1", "input": {"path": "src/a.rs"}})),
            llm(json!({"kind": "llm", "llm": "tool_started", "tool_id": "toolu_n1"})),
        ],
    );
    settle(cx, w, 8);
    let text = view.read_with(cx, |v, _| v.rail_now().text());
    assert!(text.starts_with("read") && text.contains("src/a.rs"), "NOW: {text}");
    assert!(has(cx, w, format!("agent-now {text}")));
    assert!(has(cx, w, "agent-now-life Run".into()), "streaming: NOW's glyph runs on the beat");
    // The rail adds no frame of its own while the turn runs: its glyphs
    // are on the beat's layer.
    let (open, folded) = rest(cx, w, &view);
    assert_eq!(open, folded, "the rail asks for no frame of its own");
    heard(
        cx,
        w,
        &view,
        vec![
            llm(json!({"kind": "llm", "llm": "tool_result", "tool_id": "toolu_n1", "result": "ok"})),
            llm(json!({"kind": "session", "session": "done"})),
            json!({"ev": "idle"}),
        ],
    );
    super::answer_tests::land(
        cx,
        w,
        &view,
        vec![
            json!({"role": "user", "content": "go on"}),
            json!({"role": "assistant", "content": [{"type": "tool_use", "id": "toolu_n1", "name": "read", "input": {"path": "src/a.rs"}}]}),
            json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_n1", "content": "ok"}]}),
        ],
    );
    wait(cx, w, &view, 3000);
    assert!(has(cx, w, "agent-now idle".into()) && has(cx, w, "agent-now-life Done".into()), "idle again: still");
    // The landed turn's own glow over, at rest: none at all.
    let (open, _) = rest(cx, w, &view);
    assert_eq!(open, 0, "idle: no frames, the panel open");
}

#[gpui_kit::test]
fn a_section_arriving_glides_open_and_one_leaving_glides_shut(cx: &mut TestAppContext) {
    let (w, view) = rail(cx);
    go(cx, w, &view);
    away(cx, w);
    let full = glide(cx, w, "file");
    // A section put second in the upper slot takes a place: file, the
    // agent's last open, gives way. One glides open, the other shut.
    let add = json!([{"op": "put", "id": "extra", "kind": "status", "title": "Extra", "target": "rail", "slot": "upper", "at": {"after": "plan"}, "body": {"text": "one more"}}]);
    ops(cx, w, &view, "toolu_g1", add);
    let (h0, f0) = (glide(cx, w, "extra"), glide(cx, w, "file"));
    wait(cx, w, &view, 80);
    let (hm, fm) = (glide(cx, w, "extra"), glide(cx, w, "file"));
    wait(cx, w, &view, 1000);
    let h1 = glide(cx, w, "extra");
    assert!(h0 < hm && hm < h1, "extra glides open: {h0} → {hm} → {h1}");
    assert!((f0 - full).abs() <= 0.5 && 0. < fm && fm < f0, "file glides shut: {f0} → {fm}");
    assert!(!has_sec(cx, w, "file") && more(cx, &view, 0).as_deref() == Some("+1 more · file"), "then gone, in the +N row");
    // Removed: extra glides shut; file has its place back, gliding open.
    let gone = json!([{"op": "remove", "id": "extra"}]);
    ops(cx, w, &view, "toolu_g2", gone);
    wait(cx, w, &view, 80);
    let (hm, fm) = (glide(cx, w, "extra"), glide(cx, w, "file"));
    assert!(0. < hm && hm < h1, "extra glides shut: {hm} of {h1}");
    assert!(0. < fm && fm < full, "file glides back open: {fm} of {full}");
    wait(cx, w, &view, 1000);
    assert!(!has_sec(cx, w, "extra"), "extra gone");
    assert!((glide(cx, w, "file") - full).abs() <= 0.5, "file open");
    // Reduced motion: at once.
    super::motion_level(cx, &view, crate::motion::Level::Reduced);
    let add = json!([{"op": "put", "id": "extra2", "kind": "status", "title": "Extra", "target": "rail", "slot": "upper", "at": {"after": "plan"}, "body": {"text": "again"}}]);
    ops(cx, w, &view, "toolu_g3", add);
    assert!(!has_sec(cx, w, "file"), "Reduced: file gone at once");
    assert!(glide(cx, w, "extra2") > 0., "and extra2 there at once");
}

// -- Shady #104 round 2 --------------------------------------------------------

/// P2-1: the rail's renderer state keeps to what it draws. A thousand
/// sections come and go in one session (a constant board: each round
/// removes the last one and puts the next): its painted targets and the
/// chrome's reveals stay where they began, a section leaving keeps its
/// own until its glide is over, and none outlive the session on screen.
#[gpui_kit::test]
fn the_rails_caches_keep_to_what_it_draws(cx: &mut TestAppContext) {
    let (w, view) = rail(cx);
    go(cx, w, &view);
    away(cx, w);
    let kept = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.rail_retained());
    let put = |k: usize| {
        let mut o = vec![
            json!({"op": "put", "id": format!("c{k}"), "kind": "status", "title": "C", "target": "rail", "slot": "upper", "at": {"after": "plan"}, "body": {"text": format!("round {k}")}}),
        ];
        if k > 0 {
            o.insert(0, json!({"op": "remove", "id": format!("c{}", k - 1)}));
        }
        Value::Array(o)
    };
    // Full: one round in, its glides over.
    ops(cx, w, &view, "toolu_c0", put(0));
    wait(cx, w, &view, 1000);
    let base = kept(cx);
    assert!(base.0 > 0 && base.1 > 0, "the rail's targets and reveals: {base:?}");
    // Leaving, c0 keeps its records while it glides shut, then gives them up.
    ops(cx, w, &view, "toolu_c1", put(1));
    wait(cx, w, &view, 80);
    assert!(has_sec(cx, w, "c0"), "c0 glides shut");
    wait(cx, w, &view, 1000);
    assert!(!has_sec(cx, w, "c0"));
    assert_eq!(kept(cx), base, "c0's records went with its glide");
    // Reduced: a thousand rounds at once (the records are made the same way).
    super::motion_level(cx, &view, crate::motion::Level::Reduced);
    for k in 2..1002 {
        ops(cx, w, &view, &format!("toolu_c{k}"), put(k));
        let now = kept(cx);
        assert!(now.0 <= base.0 && now.1 <= base.1, "round {k}: {now:?}, began at {base:?}");
    }
    // The last one removed: file has its place back, and the upper slot's
    // `+N` row goes.
    ops(cx, w, &view, "toolu_cz", json!([{"op": "remove", "id": "c1001"}]));
    let end = kept(cx);
    assert!(end.0 < base.0 && end.1 <= base.1, "retired: {end:?} of {base:?}");
    // Another session on screen: none of this one's.
    open_session(cx, w, &view, crate::demo::SID, Place::Current);
    wait(cx, w, &view, 1000);
    assert_eq!(kept(cx), (0, 0), "the session's rail state went with it");
}

/// The panel at its 296 px floor (1600 px wide: the right panel's default).
fn floor(cx: &mut TestAppContext) -> W {
    let (w, view) = rail_at(cx, 1600., TALL);
    let zone = f32::from(drawn(cx, w, "agent-zone 0".into()).size.width);
    assert!((zone - crate::app::FLOORS.1).abs() <= 1., "the floor: {zone}");
    (w, view)
}

fn note_h(cx: &mut TestAppContext, w: AnyWindowHandle, name: &str) -> f32 {
    f32::from(drawn(cx, w, format!("agent-note {}", row(name))).size.height)
}

/// P2-2: a note's height is its text as laid out, not an estimate. At the
/// floor its text has 226 px: two texts of 56 characters, one 2 lines as
/// laid out and the other 3 (the same count, other words). The patch from
/// one to the other moves what is under the pointer, so it waits.
#[gpui_kit::test]
fn a_patch_of_the_same_length_that_wraps_longer_waits_under_the_pointer(cx: &mut TestAppContext) {
    let (w, view) = floor(cx);
    go(cx, w, &view);
    away(cx, w);
    let two = format!("{} {}", "x".repeat(27), "x".repeat(28));
    let three = format!("{} {} {}", "x".repeat(20), "x".repeat(20), "x".repeat(14));
    assert_eq!(two.len(), three.len());
    let put = json!([{"op": "put", "id": "wrap", "kind": "note", "title": "Wrap", "target": "rail", "slot": "upper", "at": {"after": "plan"}, "body": {"text": two}}]);
    ops(cx, w, &view, "toolu_w1", put);
    wait(cx, w, &view, 1000);
    assert_eq!(note_h(cx, w, "wrap"), 40., "two lines as laid out");
    let laid = view.read_with(cx, |v, _| v.rail_text_w());
    let nw = f32::from(drawn(cx, w, format!("agent-note {}", row("wrap"))).size.width);
    assert_eq!(nw, laid, "the text is as wide as the rail lays it out");
    let at = head(cx, w, "cause");
    let y0 = sec(cx, w, "cause").top();
    mouse(cx, w, at, "move");
    ops(cx, w, &view, "toolu_w2", json!([{"op": "patch", "id": "wrap", "set": {"text": three}}]));
    assert!(waiting(cx, &view), "a patch that lays out a line longer waits");
    assert_eq!(sec(cx, w, "cause").top(), y0, "cause stays under the pointer");
    away(cx, w);
    wait(cx, w, &view, 1000);
    assert!(!waiting(cx, &view));
    assert_eq!(note_h(cx, w, "wrap"), 60., "then it lands: three lines");
}

/// P2-2: a note longer than its lines as laid out is cut there, with
/// `more`, at the floor and wide: seven lines that each wrap to two (244
/// bytes at the floor) are fourteen, not seven.
#[gpui_kit::test]
fn a_note_past_its_lines_as_laid_out_is_cut_with_more_at_296_and_520(cx: &mut TestAppContext) {
    let (w, view) = floor(cx);
    go(cx, w, &view);
    away(cx, w);
    let lines = |each: usize| vec!["W ".repeat(each); 7].join("\n");
    let narrow = lines(17);
    assert_eq!(narrow.len(), 244);
    let put = json!([{"op": "put", "id": "tall", "kind": "note", "title": "Tall", "target": "rail", "slot": "upper", "at": {"after": "plan"}, "body": {"text": narrow}}]);
    ops(cx, w, &view, "toolu_t1", put);
    wait(cx, w, &view, 1000);
    let cut = 20. * crate::agent_rail::NOTE_LINES as f32;
    let more = |cx: &mut TestAppContext, word: &str| has(cx, w, format!("agent-note-more {} {word}", row("tall")));
    assert_eq!(note_h(cx, w, "tall"), cut, "296: fourteen lines as laid out, cut");
    assert!(more(cx, "more"), "296: `more`");
    // Opened: all fourteen.
    let at = drawn(cx, w, format!("agent-note-more {} more", row("tall"))).center();
    click(cx, w, at);
    wait(cx, w, &view, 1000);
    assert_eq!(note_h(cx, w, "tall"), 280., "opened whole");
    assert!(more(cx, "less"));
    let at = drawn(cx, w, format!("agent-note-more {} less", row("tall"))).center();
    click(cx, w, at);
    wait(cx, w, &view, 1000);
    // Wide (520 px: 450 px of text): the same text is seven lines, whole.
    away(cx, w);
    view.update(cx, |v, cx| {
        let p = crate::theme::Panels { right_width: Some(520), ..v.panels };
        v.set_panels_pub(p, cx);
    });
    wait(cx, w, &view, 1500);
    assert_eq!(note_h(cx, w, "tall"), 140., "520: seven lines");
    assert!(!more(cx, "more") && !more(cx, "less"), "520: no `more`");
    // Seven lines of 60 characters: fourteen at 520, cut.
    ops(cx, w, &view, "toolu_t2", json!([{"op": "patch", "id": "tall", "set": {"text": lines(30)}}]));
    wait(cx, w, &view, 1000);
    assert_eq!(note_h(cx, w, "tall"), cut, "520: fourteen lines as laid out, cut");
    assert!(more(cx, "more"), "520: `more`");
}

fn head_y(cx: &mut TestAppContext, w: AnyWindowHandle, name: &str) -> f32 {
    f32::from(drawn(cx, w, format!("agent-head {}", row(name))).top())
}

/// An agent op `ops` that glides moves `target`'s header. Full: pressed
/// mid-glide, the pointer holds the glide (its header stays where the
/// pointer found it) and the press does nothing, once; the glide lands
/// once the pointer leaves, and a press just after does nothing once too
/// (the window runs from the motion's end); 700 ms on, a click acts.
/// Reduced and Off: it moves at once, and a press just after does nothing
/// once.
fn moved_by(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, tool: &str, o: Value, target: &str, fluid: bool) {
    away(cx, w);
    let y = head_y(cx, w, target);
    ops(cx, w, view, tool, o);
    let was = open(cx, w, target);
    if fluid {
        wait(cx, w, view, 80);
        let at = head(cx, w, target);
        mouse(cx, w, at, "move");
        let y0 = head_y(cx, w, target);
        assert!((y0 - y).abs() > 0.5, "{tool}: mid-glide, {target} on its way ({y} → {y0})");
        for k in 0..6 {
            wait(cx, w, view, 40);
            mouse(cx, w, at, "move");
            assert_eq!(head_y(cx, w, target), y0, "{tool}: held under the pointer ({k})");
        }
        mouse(cx, w, at, "down");
        mouse(cx, w, at, "up");
        assert_eq!(says(cx, view), Some((row(target), crate::lease::MOVED)), "{tool}: pressed mid-glide, nothing");
        assert_eq!(open(cx, w, target), was, "{tool}: nothing folded");
        away(cx, w);
        wait(cx, w, view, 1000);
    }
    assert!((head_y(cx, w, target) - y).abs() > 0.5, "{tool}: {target} moved");
    let at = head(cx, w, target);
    click(cx, w, at);
    assert_eq!(says(cx, view), Some((row(target), crate::lease::MOVED)), "{tool}: just after the glide, nothing");
    assert_eq!(open(cx, w, target), was, "{tool}: nothing folded");
    away(cx, w);
    wait(cx, w, view, 700);
    let at = head(cx, w, target);
    click(cx, w, at);
    wait(cx, w, view, 1000);
    assert_eq!(open(cx, w, target), !was, "{tool}: 700 ms on, the click acts");
    // Back as it was.
    let at = head(cx, w, target);
    click(cx, w, at);
    wait(cx, w, view, 1000);
    away(cx, w);
}

/// P2-3: an agent op's glides keep their provenance to their end: a
/// section arriving above, one leaving, one folding (the agent's
/// collapse), each moving `cause` below it.
#[gpui_kit::test]
fn what_an_agent_glide_moves_is_just_moved_to_its_end_and_held_under_the_pointer(cx: &mut TestAppContext) {
    let (w, view) = rail(cx);
    go(cx, w, &view);
    let arrive = json!([{"op": "put", "id": "extra", "kind": "status", "title": "Extra", "target": "rail", "slot": "upper", "at": {"after": "plan"}, "body": {"text": "one more"}}]);
    moved_by(cx, w, &view, "toolu_p1", arrive, "cause", true);
    moved_by(cx, w, &view, "toolu_p2", json!([{"op": "collapse", "id": "plan"}]), "cause", true);
    moved_by(cx, w, &view, "toolu_p3", json!([{"op": "remove", "id": "extra"}]), "cause", true);
    assert_eq!(settle(cx, w, 8), 0, "at rest");
}

#[gpui_kit::test]
fn what_an_agent_op_moves_at_once_is_just_moved_reduced_and_off(cx: &mut TestAppContext) {
    for (k, level) in [crate::motion::Level::Reduced, crate::motion::Level::Off].into_iter().enumerate() {
        let (w, view) = rail(cx);
        super::motion_level(cx, &view, level);
        go(cx, w, &view);
        let arrive = json!([{"op": "put", "id": "extra", "kind": "status", "title": "Extra", "target": "rail", "slot": "upper", "at": {"after": "plan"}, "body": {"text": "one more"}}]);
        moved_by(cx, w, &view, &format!("toolu_q{k}1"), arrive, "cause", false);
        moved_by(cx, w, &view, &format!("toolu_q{k}2"), json!([{"op": "collapse", "id": "plan"}]), "cause", false);
        moved_by(cx, w, &view, &format!("toolu_q{k}3"), json!([{"op": "remove", "id": "extra"}]), "cause", false);
    }
}

/// No control, bidi or invisible character, nor a line break.
fn clean(s: &str) -> bool {
    !s.chars().any(|c| c.is_control() || cards_spec::sanitize::is_invisible(c) || matches!(c, '\u{2028}' | '\u{2029}'))
}

/// P2-4: NOW draws the agent's strings as the desk's sanitizer has them:
/// a read's path, a search's pattern, an extension tool's name and a
/// prompt's scope, each with bidi overrides and isolates, C0 and C1
/// controls and line breaks; its own counts whole.
#[gpui_kit::test]
fn now_draws_tool_and_prompt_strings_sanitized(cx: &mut TestAppContext) {
    let (w, view) = rail(cx);
    go(cx, w, &view);
    let call = |id: &str, name: &str, input: Value| {
        vec![
            llm(json!({"kind": "llm", "llm": "tool_use_start", "tool_name": name, "tool_id": id})),
            llm(json!({"kind": "llm", "llm": "tool_use", "tool_name": name, "tool_id": id, "input": input})),
            llm(json!({"kind": "llm", "llm": "tool_started", "tool_id": id})),
        ]
    };
    let mut evs = vec![llm(json!({"kind": "llm", "llm": "response_start"}))];
    evs.extend(call("toolu_s1", "read", json!({"path": "src/\u{202E}evil\u{202C}.rs\nmore"})));
    evs.extend(call("toolu_s2", "grep", json!({"pattern": "a\u{0007}b\u{2066}c\u{2069}\td", "path": "."})));
    evs.extend(call("toolu_s3", "ext\u{0085}scan\u{200F}\u{001B}[2J", json!({"q": "x\u{2028}y"})));
    heard(cx, w, &view, evs);
    settle(cx, w, 8);
    let now = |cx: &mut TestAppContext| {
        let text = view.read_with(cx, |v, _| v.rail_now().text());
        assert!(clean(&text), "NOW sanitized: {text:?}");
        assert!(has(cx, w, format!("agent-now {text}")), "drawn as {text:?}");
        text
    };
    let first = now(cx);
    assert!(first.starts_with("read src/evil.rs") && first.ends_with("  +2 running"), "{first:?}");
    let done = |id: &str| llm(json!({"kind": "llm", "llm": "tool_result", "tool_id": id, "result": "ok"}));
    heard(cx, w, &view, vec![done("toolu_s1")]);
    let second = now(cx);
    assert!(second.starts_with("grep") && second.ends_with("  +1 running"), "{second:?}");
    heard(cx, w, &view, vec![done("toolu_s2")]);
    let third = now(cx);
    assert!(third.starts_with("extscan"), "{third:?}");
    heard(cx, w, &view, vec![done("toolu_s3")]);
    let prompt = json!({"ev": "prompt", "request": {"id": 9, "kind": "confirm", "title": "Confirm tool call", "prompt": "p",
        "confirm_scope": {"extensions": ["guard"], "tool": "ed\u{2066}it\u{0000}\u{009B}"}}});
    heard(cx, w, &view, vec![prompt]);
    assert_eq!(now(cx), "waiting for you · allow edit");
}

/// A visibility of D6b's (the seam): `n1` softly dismissed (its
/// tombstone), `n3` dismissed.
struct Soft;

impl crate::agent_rail::RailVisibility for Soft {
    fn seen(&self, id: &Ref) -> crate::agent_rail::Seen {
        match id.to_string().as_str() {
            "n1" => crate::agent_rail::Seen::Tombstone(crate::agent_rail::Tomb {
                text: "dismissed before compaction · u show".into(),
                token: 7,
            }),
            "n3" => crate::agent_rail::Seen::Hidden,
            _ => crate::agent_rail::Seen::Shown,
        }
    }
}

/// P2-5: a group's member the visibility says is a tombstone draws as
/// its tombstone, never its content (an agent change to it included); a
/// hidden member leaves the group; a shown one draws as itself.
#[gpui_kit::test]
fn a_group_members_tombstone_draws_as_one_never_its_content(cx: &mut TestAppContext) {
    let (w, view) = rail(cx);
    view.update(cx, |v, cx| {
        v.agent_rail.vis = Some(std::rc::Rc::new(Soft));
        cx.notify();
    });
    go(cx, w, &view);
    away(cx, w);
    let add = json!([
        {"op": "put", "id": "n3", "kind": "note", "title": "Hidden", "target": "rail", "slot": "end", "body": {"text": "not drawn"}},
        {"op": "merge", "into": "findings", "ids": ["n3"]}
    ]);
    ops(cx, w, &view, "toolu_v1", add);
    let at = head(cx, w, "findings");
    click(cx, w, at);
    wait(cx, w, &view, 1000);
    assert!(open(cx, w, "findings"), "the group open");
    let g = row("findings");
    let tomb = format!("agent-member-tomb {g} {} dismissed before compaction · u show", row("n1"));
    let member = |cx: &mut TestAppContext, n: &str| has(cx, w, format!("agent-member {g} {}", row(n)));
    assert!(has(cx, w, tomb.clone()), "n1 draws as its tombstone");
    assert!(!member(cx, "n1"), "never its content");
    assert!(member(cx, "n2"), "n2 as itself");
    assert!(!member(cx, "n3") && !has(cx, w, format!("agent-member-tomb {g} {}", row("n3"))), "n3 hidden");
    // The agent changes n1: still its tombstone.
    away(cx, w);
    ops(cx, w, &view, "toolu_v2", json!([{"op": "patch", "id": "n1", "set": {"text": "Changed under the tombstone"}}]));
    wait(cx, w, &view, 1000);
    assert!(has(cx, w, tomb), "n1 still its tombstone");
    assert!(!member(cx, "n1"));
    let layout = view.read_with(cx, |v, _| v.rail_layout().map(|(_, l)| l.section(&Ref::parse("findings").expect("an id")).cloned()));
    let sec = layout.flatten().expect("findings");
    let ids: Vec<String> = sec.s.members.iter().map(|m| m.block.id.to_string()).collect();
    assert_eq!(ids, ["n1", "n2"], "n3 left the group's list");
    assert_eq!(
        sec.tombs.iter().map(|t| t.as_ref().map(|t| (t.text.to_string(), t.token))).collect::<Vec<_>>(),
        vec![Some(("dismissed before compaction · u show".to_string(), 7)), None]
    );
}

/// P3: a header's controls keep their place at the floor and wide: a
/// replaced block past a thousand revisions keeps its chip, its revision
/// (cut, whole in its tooltip) and its chevron inside the header.
#[gpui_kit::test]
fn a_long_revision_keeps_the_headers_controls_inside_at_296_and_520(cx: &mut TestAppContext) {
    let (w, view) = floor(cx);
    go(cx, w, &view);
    away(cx, w);
    let put = json!([{"op": "put", "id": "big", "kind": "status", "title": "A long title for a status", "target": "rail", "slot": "upper", "at": {"after": "plan"}, "body": {"text": "r0"}}]);
    ops(cx, w, &view, "toolu_b0", put);
    let again = json!([{"op": "replace", "id": "big", "kind": "status", "why": "again", "body": {"text": "again"}}]);
    ops(cx, w, &view, "toolu_b1", again);
    let calls: Vec<(String, Value)> = (0..63)
        .map(|k| {
            let o: Vec<Value> = (0..16).map(|j| json!({"op": "patch", "id": "big", "set": {"text": format!("{k}.{j}")}})).collect();
            (format!("toolu_bp{k}"), Value::Array(o))
        })
        .collect();
    let calls: Vec<(&str, Value, bool)> = calls.iter().map(|(t, o)| (t.as_str(), o.clone(), true)).collect();
    round(cx, w, &view, &calls);
    wait(cx, w, &view, 1000);
    let rev = view.read_with(cx, |v, _| {
        v.rail_layout()
            .and_then(|(_, l)| l.section(&Ref::parse("big").expect("an id")).map(|s| (s.s.block.rev, s.s.block.replaced.is_some())))
    });
    let (n, replaced) = rev.expect("big");
    assert!(n >= 1000 && replaced, "r{n}, replaced {replaced}");
    let b = row("big");
    let inside = |cx: &mut TestAppContext, at: &str| {
        let h = drawn(cx, w, format!("agent-head {b}"));
        let chip = drawn(cx, w, format!("agent-chip {b}"));
        let chev = drawn(cx, w, format!("agent-chevron {b}"));
        let rev = drawn(cx, w, format!("agent-rev {b} r{n} · replaced"));
        for (what, r) in [("chip", chip), ("revision", rev), ("chevron", chev)] {
            assert!(
                r.left() >= h.left() - px(0.5) && r.right() <= h.right() + px(0.5),
                "{at}: the {what} inside the header: {r:?} in {h:?}"
            );
        }
        assert!((f32::from(chev.size.width) - 10.).abs() <= 0.5, "{at}: the chevron's 10 px");
        assert!(f32::from(chip.size.width) >= 40., "{at}: the chip whole: {chip:?}");
        assert!(chip.right() <= rev.left() && rev.right() <= chev.left() + px(0.5), "{at}: in order");
    };
    inside(cx, "296");
    view.update(cx, |v, cx| {
        let p = crate::theme::Panels { right_width: Some(520), ..v.panels };
        v.set_panels_pub(p, cx);
    });
    wait(cx, w, &view, 1500);
    inside(cx, "520");
}
