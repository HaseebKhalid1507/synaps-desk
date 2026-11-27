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
