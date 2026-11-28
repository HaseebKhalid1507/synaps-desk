//! The edit guard on Settings' two short fields (settings-overhaul PLAN
//! §3.6 Part 5, §4 PR 5's kit-input gates): their caps, and their undo
//! histories held to their budgets. Memory is read on the test thread's
//! own account of the counting allocator (`heap`): GPUI runs a test's
//! tasks, the background ones too, on its thread.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::component::input::{InputEditsEvent, Paste, Redo, SelectAll, Undo};
use gpui_kit::{
    Action, AnyWindowHandle, AppContext as _, ClipboardItem, Entity, EntityId, EntityInputHandler as _, TestAppContext, VisualTestContext,
};

use super::keys_tests::press;
use super::settings_continuity_tests::{NARROW, Page, click, drawn, find, open_settings, page};
use super::settle;
use crate::app::AppView;
use crate::settings_editor::{CLEARED, EditGuard, FIND_BUDGET, FIND_CAP, Field, VALUE_BUDGET, VALUE_CAP};

// ── the fixture ───────────────────────────────────────────────────────

fn act(cx: &mut TestAppContext, w: AnyWindowHandle, a: impl Action) {
    cx.update_window(w, |_, window, cx| window.dispatch_action(Box::new(a), cx)).expect("the window");
    cx.run_until_parked();
    settle(cx, w, 4);
}

/// A select-all paste of `text` into the field focused.
fn paste(cx: &mut TestAppContext, w: AnyWindowHandle, text: &str) {
    cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
    act(cx, w, SelectAll);
    act(cx, w, Paste);
}

fn input(cx: &mut TestAppContext, pg: &Page, f: Field) -> Entity<gpui_kit::component::input::InputState> {
    pg.view.read_with(cx, |v, _| v.field_input(f).clone())
}

fn text(cx: &mut TestAppContext, pg: &Page, f: Field) -> String {
    let i = input(cx, pg, f);
    i.read_with(cx, |s, _| s.value().to_string())
}

fn charge(cx: &mut TestAppContext, pg: &Page, f: Field) -> u32 {
    pg.view.update(cx, |v, _| v.field_guard(f).charge())
}

/// An IME's preedit of `text` into field `f`, through the input handler
/// the platform calls (`replace_and_mark_text_in_range`, over what is
/// marked, else the selection), then two frames.
fn mark(cx: &mut TestAppContext, pg: &Page, f: Field, text: &str) {
    let i = input(cx, pg, f);
    cx.update_window(pg.w, |_, window, cx| i.update(cx, |s, cx| s.replace_and_mark_text_in_range(None, text, None, window, cx)))
        .expect("the window");
    cx.run_until_parked();
    settle(cx, pg.w, 2);
}

/// The composition committed as `text` (`replace_text_in_range`).
fn commit(cx: &mut TestAppContext, pg: &Page, f: Field, text: &str) {
    let i = input(cx, pg, f);
    cx.update_window(pg.w, |_, window, cx| i.update(cx, |s, cx| s.replace_text_in_range(None, text, window, cx))).expect("the window");
    cx.run_until_parked();
    settle(cx, pg.w, 2);
}

/// Field `f`'s marked range (UTF-16), if a composition is under way.
fn marked(cx: &mut TestAppContext, pg: &Page, f: Field) -> Option<std::ops::Range<usize>> {
    let i = input(cx, pg, f);
    cx.update_window(pg.w, |_, window, cx| i.update(cx, |s, cx| s.marked_text_range(window, cx))).expect("the window")
}

/// The dash URL field open, holding `value`.
fn url_field(cx: &mut TestAppContext, name: &str, value: &str) -> Page {
    let pg = page(cx, name, NARROW);
    pg.view.update(cx, |v, _| v.app_settings.dash_url = Some(value.to_string()));
    find(cx, pg.w, "dash_url");
    click(cx, pg.w, "field-dash-url");
    assert_eq!(pg.view.read_with(cx, |v, _| v.settings.editing), Some(crate::settings_view::Edit::DashUrl), "open");
    assert!(text(cx, &pg, Field::Value) == value, "opened on its value");
    pg
}

/// The find box focused, empty.
fn find_field(cx: &mut TestAppContext, name: &str) -> Page {
    let pg = page(cx, name, NARROW);
    click(cx, pg.w, "settings-find");
    pg
}

/// What this test's account holds, a second on and two frames on (the
/// window's line cache turned over), less the app's test-only frame logs.
/// - A second: the test clock moves only when a test moves it, so every
///   timer an edit started waits until then. Each edit pauses the caret's
///   blink (gpui-base 0.9.1 `BlinkCursor::schedule`), whose timer, in an app,
///   fires within half a second.
/// - The logs: each frame, under test, the app logs its chrome's boxes
///   (`AppView::probe`) and each scroll bar its paints, kept until a test
///   reads them. A build that ships has none, so they are dropped first.
fn held(cx: &mut TestAppContext, pg: &Page) -> isize {
    cx.executor().advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    for _ in 0..2 {
        cx.update_window(pg.w, |_, window, _| window.refresh()).expect("the window");
        settle(cx, pg.w, 4);
    }
    pg.view.read_with(cx, |v: &AppView, _| {
        drop(v.chrome_log.take());
        let b = &v.bars;
        for s in [&b.transcript, &b.rail, &b.right.bar, &b.picker.bar, &b.settings, &b.prompt.bar, &b.usage, &b.palette] {
            s.drain_test_logs();
        }
    });
    crate::heap::held()
}

/// The size of GPUI's frame profiler for window `w` (the workspace builds
/// gpui-kit with `profiler`): its histograms' counts, in values. They are
/// on the test thread's account, and grow (never shrink) by 8 KiB a bucket
/// whenever a frame's real time sets a new maximum (`gpui-pre 0.3.12
/// src/profiler.rs:1193–1257`, `Histogram::new(3)` auto-resizing), so a
/// measurement they grew in says nothing of the field.
fn profiler_size(cx: &mut TestAppContext, w: AnyWindowHandle) -> usize {
    cx.update_window(w, |_, window, _| {
        let f = window.frame_duration_snapshot();
        let i = window.input_latency_snapshot();
        let by_interval: usize = f
            .by_refresh_interval
            .iter()
            .map(|r| r.draw_duration.distinct_values() + r.work.distinct_values() + r.input_work.distinct_values())
            .sum();
        f.dirty_to_present_histogram.distinct_values()
            + f.draw_duration_histogram.distinct_values()
            + f.present_interval_histogram.distinct_values()
            + i.latency_histogram.distinct_values()
            + i.events_per_frame_histogram.distinct_values()
            + by_interval
            + f.by_refresh_interval.len()
    })
    .expect("the window")
}

/// Frames drawn until 40 more hold within 1 KiB: what the window keeps of
/// its draws (GPUI's frame profiler among it) at its high-water mark, so a
/// run measured from here pays none of it. At most 40 rounds.
fn steady(cx: &mut TestAppContext, pg: &Page) {
    for _ in 0..40 {
        let start = held(cx, pg);
        for _ in 0..20 {
            held(cx, pg);
        }
        if held(cx, pg) - start <= 1024 {
            return;
        }
    }
    panic!("the window's account never held steady");
}

/// Field `f`'s undo history, in bytes: what the account holds less what it
/// holds once the input is rebuilt from its own text.
fn history(cx: &mut TestAppContext, pg: &Page, f: Field) -> isize {
    let before = held(cx, pg);
    cx.update_window(pg.w, |_, window, cx| pg.view.update(cx, |v, cx| v.rebuild_field(f, window, cx))).expect("the window");
    before - held(cx, pg)
}

/// The kit's fixed part per step (`T`): what 1,000 one-character steps of
/// the value field hold, less their text, per step. Measured once.
fn fixed_per_step(cx: &mut TestAppContext) -> isize {
    let pg = url_field(cx, "fixed", "");
    // Its budget lifted, so no rebuild clears what is measured.
    pg.view.update(cx, |v, _| v.settings_input_guard = crate::settings_editor::EditGuard::new(VALUE_CAP, u32::MAX));
    {
        let i = input(cx, &pg, Field::Value);
        for _ in 0..1000 {
            cx.update_window(pg.w, |_, window, cx| i.update(cx, |s, cx| s.insert("x", window, cx))).expect("the window");
        }
    }
    cx.run_until_parked();
    let h = history(cx, &pg, Field::Value);
    eprintln!("KIT-FIXED-RAW: 1,000 one-character steps hold {h} B");
    (h - 1000).max(0) / 1000 + 1
}

/// One step of a churn.
#[derive(Clone, Copy, Debug)]
enum Op {
    Paste(usize),
    Undo,
    Redo,
    /// Select all and type 300 characters.
    Type,
}

/// PLAN §4's sequence: 20 select-all pastes, an undo and a redo after the
/// 5th, two undos and two redos after the 12th, then select all and type.
fn churn_ops() -> Vec<Op> {
    let mut ops = Vec::new();
    for i in 1..=20 {
        ops.push(Op::Paste(i));
        match i {
            5 => ops.extend([Op::Undo, Op::Redo]),
            12 => ops.extend([Op::Undo, Op::Undo, Op::Redo, Op::Redo]),
            _ => {}
        }
    }
    ops.push(Op::Type);
    ops
}

/// Paste `i`'s text, `len` bytes, each its own.
fn nth(i: usize, len: usize) -> String {
    let head = format!("{i:03}");
    format!("{head}{}", "v".repeat(len - head.len()))
}

/// What a churn left: the steps since the last rebuild, the rebuilds, and
/// every input the field had.
struct Churned {
    steps: isize,
    rebuilds: usize,
    inputs: Vec<gpui_kit::WeakEntity<gpui_kit::component::input::InputState>>,
}

/// `ops` on field `f` (each paste `len` bytes), checked after each: the
/// charge within `budget`, the text the history model says (a rebuild
/// leaves it, and clears the model's history), a paste's caret at its end,
/// and the note once a rebuild has run.
fn churn(cx: &mut TestAppContext, pg: &Page, f: Field, ops: &[Op], len: usize, budget: u32) -> Churned {
    let (mut undo, mut redo): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
    let mut cur = text(cx, pg, f);
    let mut out = Churned { steps: 0, rebuilds: 0, inputs: vec![input(cx, pg, f).downgrade()] };
    for (n, op) in ops.iter().enumerate() {
        let was: EntityId = input(cx, pg, f).entity_id();
        let before = cur.clone();
        match *op {
            Op::Paste(i) => {
                paste(cx, pg.w, &nth(i, len));
                undo.push(std::mem::replace(&mut cur, nth(i, len)));
                redo.clear();
                out.steps += 1;
            }
            Op::Undo => {
                act(cx, pg.w, Undo);
                if let Some(p) = undo.pop() {
                    redo.push(std::mem::replace(&mut cur, p));
                }
            }
            Op::Redo => {
                act(cx, pg.w, Redo);
                if let Some(x) = redo.pop() {
                    undo.push(std::mem::replace(&mut cur, x));
                }
            }
            Op::Type => {
                // Typed past the field's cap, the rest is refused.
                act(cx, pg.w, SelectAll);
                VisualTestContext::from_window(pg.w, cx).simulate_input(&"y".repeat(300));
                settle(cx, pg.w, 4);
                undo.push(std::mem::replace(&mut cur, "y".repeat(300.min(len))));
                redo.clear();
                out.steps += 1;
            }
        }
        if input(cx, pg, f).entity_id() != was {
            // Rebuilt: the text as it was, the history gone. A paste's
            // rebuild came first, so the paste is its one step, and its
            // undo restores the value from before it.
            out.rebuilds += 1;
            out.inputs.push(input(cx, pg, f).downgrade());
            undo.clear();
            redo.clear();
            out.steps = if matches!(op, Op::Paste(_)) {
                undo.push(before);
                1
            } else {
                0
            };
        }
        let c = charge(cx, pg, f);
        assert!(c <= budget, "after {op:?} (step {n}): charge {c} > {budget}");
        let now = text(cx, pg, f);
        assert!(
            now == cur,
            "after {op:?} (step {n}): the text is {:?}…, {} B; the model's {:?}…, {} B",
            &now[..now.len().min(8)],
            now.len(),
            &cur[..cur.len().min(8)],
            cur.len()
        );
        if let Op::Paste(_) = op {
            let i = input(cx, pg, f);
            let sel = i.read_with(cx, |s, _| s.selected_range());
            assert_eq!(sel, cur.len()..cur.len(), "after {op:?}: the caret at the paste's end");
        }
    }
    if out.rebuilds > 0 {
        let notes = pg.view.read_with(cx, |v, _| v.field_note(f));
        assert_eq!(notes.as_deref(), Some(CLEARED), "the note, once");
    }
    out
}

// ── the field limit ───────────────────────────────────────────────────

/// A 4,097 B paste into the dash URL field is refused whole: the field as
/// it was, the note under it, and nothing added to its history (one undo
/// undoes the edit before it).
#[gpui_kit::test]
fn field_limit_refuses_a_long_paste_whole(cx: &mut TestAppContext) {
    let pg = url_field(cx, "long-paste", "http://127.0.0.1:7999");
    let i = input(cx, &pg, Field::Value);
    cx.update_window(pg.w, |_, window, cx| i.update(cx, |s, cx| s.insert("/x", window, cx))).expect("the window");
    settle(cx, pg.w, 4);
    let before = text(cx, &pg, Field::Value);
    assert_eq!(before, "http://127.0.0.1:7999/x");
    let charged = charge(cx, &pg, Field::Value);
    cx.write_to_clipboard(ClipboardItem::new_string("p".repeat(VALUE_CAP + 1)));
    act(cx, pg.w, Paste);
    assert_eq!(text(cx, &pg, Field::Value), before, "refused whole");
    assert_eq!(charge(cx, &pg, Field::Value), charged, "nothing charged");
    assert!(drawn(cx, pg.w, "field-note Not pasted: this field takes at most 4,096 bytes."), "the note");
    act(cx, pg.w, Undo);
    assert_eq!(text(cx, &pg, Field::Value), "http://127.0.0.1:7999", "one undo: the edit before it");
}

/// A value longer than the field takes (set elsewhere) shows its length
/// and where to edit it, and no field opens.
#[gpui_kit::test]
fn a_value_past_the_field_limit_does_not_open_the_field(cx: &mut TestAppContext) {
    let pg = page(cx, "long-value", NARROW);
    pg.view.update(cx, |v, _| v.app_settings.daemon_label = Some("j".repeat(5000)));
    find(cx, pg.w, "daemon_label");
    assert!(drawn(cx, pg.w, "field-daemon-label = 5,000 bytes: longer than this app edits; use /settings in the TUI"));
    click(cx, pg.w, "field-daemon-label");
    assert_eq!(pg.view.read_with(cx, |v, _| v.settings.editing), None, "no field opens");
    assert_eq!(text(cx, &pg, Field::Value), "", "the field holds nothing of it");
}

// ── the kit's facts ───────────────────────────────────────────────────

/// The kit facts Part 5 relies on, pinned: a kit upgrade that changes one
/// turns this red. On a single-line input (the rail's find, which has no
/// edit guard to clear its history) and on a textarea (the composer): the
/// depth is 1,000 steps; a select-all paste
/// is one step; an undo reports the paste's span, inverted; `set_value`
/// leaves nothing to undo; on the single-line input a refused edit leaves
/// the text and records nothing. (The textarea has no validator to set:
/// `validate` is the single-line input's only.) And the fixed part,
/// measured: ≤ 512,000 B for 1,000 steps.
#[gpui_kit::test]
fn kit_history_is_what_the_plan_assumes(cx: &mut TestAppContext) {
    crate::heap::mine();
    // The depth: 1,100 steps, 1,100 undos, 100 left. On the value field,
    // its guard's budget lifted (so it never rebuilds), and on the
    // composer, in a window showing its session.
    let pg = url_field(cx, "kit", "");
    pg.view.update(cx, |v, _| v.settings_input_guard = crate::settings_editor::EditGuard::new(VALUE_CAP, u32::MAX));
    let value = input(cx, &pg, Field::Value);
    for _ in 0..1100 {
        cx.update_window(pg.w, |_, window, cx| value.update(cx, |s, cx| s.insert("x", window, cx))).expect("the window");
    }
    cx.run_until_parked();
    for _ in 0..1100 {
        act(cx, pg.w, Undo);
    }
    assert_eq!(text(cx, &pg, Field::Value).len(), 100, "a single-line input: a depth of 1,000");
    let (w, view, _events) = super::open_at(cx, NARROW, crate::theme::Panels::default().text_size);
    let composer = view.read_with(cx, |v, _| v.panes.cur.composer.clone());
    cx.update_window(w, |_, window, cx| composer.update(cx, |s, cx| s.focus(window, cx))).expect("the window");
    settle(cx, w, 4);
    for _ in 0..1100 {
        cx.update_window(w, |_, window, cx| composer.update(cx, |s, cx| s.insert("x", window, cx))).expect("the window");
    }
    cx.run_until_parked();
    for _ in 0..1100 {
        act(cx, w, Undo);
    }
    assert_eq!(composer.read_with(cx, |s, _| s.value().len()), 100, "the textarea: a depth of 1,000");
    view.update(cx, |v, _| v.client = None);
    // A select-all paste: one step; its undo reports its span, inverted.
    let pg = url_field(cx, "kit-paste", "abc");
    let i = input(cx, &pg, Field::Value);
    let seen: Rc<RefCell<Vec<(usize, usize, usize)>>> = Rc::default();
    let s2 = seen.clone();
    let _sub = cx.update(|cx| {
        cx.subscribe(&i, move |_, ev: &InputEditsEvent, _| {
            s2.borrow_mut().extend(ev.edits().iter().map(|(e, _)| (e.start_byte, e.old_end_byte, e.new_end_byte)));
        })
    });
    paste(cx, pg.w, "zzzzz");
    assert_eq!(text(cx, &pg, Field::Value), "zzzzz");
    act(cx, pg.w, Undo);
    assert_eq!(text(cx, &pg, Field::Value), "abc", "one undo: the whole paste");
    assert_eq!(*seen.borrow(), [(0, 3, 5), (0, 5, 3)], "the paste's span, then the undo's, inverted");
    // `set_value` leaves nothing to undo.
    cx.update_window(pg.w, |_, window, cx| i.update(cx, |s, cx| s.set_value("q", window, cx))).expect("the window");
    act(cx, pg.w, Undo);
    assert_eq!(text(cx, &pg, Field::Value), "q", "nothing to undo");
    // A refused edit (past the validator's cap) leaves the text and
    // records nothing: one undo undoes the edit before it.
    cx.update_window(pg.w, |_, window, cx| i.update(cx, |s, cx| s.insert("r", window, cx))).expect("the window");
    cx.update_window(pg.w, |_, window, cx| i.update(cx, |s, cx| s.insert("s".repeat(VALUE_CAP), window, cx))).expect("the window");
    cx.run_until_parked();
    assert_eq!(text(cx, &pg, Field::Value), "qr", "refused");
    act(cx, pg.w, Undo);
    assert_eq!(text(cx, &pg, Field::Value), "q", "the edit before it undone");
    // A composition (Shady #116 B1): no edit event for its preedits, each
    // kept until it ends, though its commit reports one whole-text span; a
    // cancel reports nothing. On the value field, its budget lifted: 40
    // distinct 4,096 B preedits over a 4,096 B value hold well past what
    // the commit's span says (2 × 8,192 + a step).
    for cancel in [false, true] {
        let pg = url_field(cx, "kit-compose", &nth(0, VALUE_CAP));
        pg.view.update(cx, |v, _| v.settings_input_guard = EditGuard::new(VALUE_CAP, u32::MAX));
        let i = input(cx, &pg, Field::Value);
        let seen: Rc<RefCell<Vec<(usize, usize, usize)>>> = Rc::default();
        let s2 = seen.clone();
        let _sub = cx.update(|cx| {
            cx.subscribe(&i, move |_, ev: &InputEditsEvent, _| {
                s2.borrow_mut().extend(ev.edits().iter().map(|(e, _)| (e.start_byte, e.old_end_byte, e.new_end_byte)));
            })
        });
        act(cx, pg.w, SelectAll);
        let before = held(cx, &pg);
        for k in 0..40 {
            mark(cx, &pg, Field::Value, &nth(k + 1, VALUE_CAP));
        }
        assert!(seen.borrow().is_empty(), "no edit event for a preedit");
        let kept = held(cx, &pg) - before;
        eprintln!("KIT-COMPOSITION: 40 preedits of 4,096 B hold {kept} B");
        assert!(kept > 40 * VALUE_CAP as isize, "each preedit kept: {kept} B");
        if cancel {
            mark(cx, &pg, Field::Value, "");
            assert_eq!(
                (text(cx, &pg, Field::Value), marked(cx, &pg, Field::Value)),
                (String::new(), None),
                "a cancel: the marked text out"
            );
            assert!(seen.borrow().is_empty(), "a cancel reports nothing");
        } else {
            commit(cx, &pg, Field::Value, "done");
            assert_eq!(*seen.borrow(), [(0, VALUE_CAP, 4)], "its commit: one span, the whole text");
            act(cx, pg.w, Undo);
            assert!(text(cx, &pg, Field::Value) == nth(0, VALUE_CAP), "one undo: the whole composition");
        }
    }
    // The fixed part.
    let t = fixed_per_step(cx);
    eprintln!("KIT-FIXED: T = {t} B a step, {} B for 1,000 steps", 1000 * t);
    assert!(1000 * t <= 512_000, "the fixed part: {} B", 1000 * t);
}

// ── the budgets ───────────────────────────────────────────────────────

/// The value field's history within its 65,536 B budget through PLAN §4's
/// churn of 4,096 B pastes: the charge after every step, and the bytes
/// held at three checkpoints (each a run of its own), within the charge
/// plus the kit's fixed part per step.
#[gpui_kit::test]
fn value_field_history_holds_its_budget(cx: &mut TestAppContext) {
    crate::heap::mine();
    let t = fixed_per_step(cx);
    let ops = churn_ops();
    let third = ops.iter().position(|o| matches!(o, Op::Paste(3))).expect("the 3rd");
    let twelfth = ops.iter().position(|o| matches!(o, Op::Paste(13))).expect("the 13th") - 1;
    for (name, upto) in [("3rd paste", third), ("12th's redos", twelfth), ("the end", ops.len() - 1)] {
        let pg = url_field(cx, "value-budget", &nth(0, VALUE_CAP));
        let c = churn(cx, &pg, Field::Value, &ops[..=upto], VALUE_CAP, VALUE_BUDGET);
        let charged = charge(cx, &pg, Field::Value) as isize;
        let h = history(cx, &pg, Field::Value);
        eprintln!("VALUE-HISTORY {name}: {h} B held, charge {charged}, {} steps, {} rebuilds", c.steps, c.rebuilds);
        assert!(h <= charged + t * c.steps, "{name}: {h} B > the charge {charged} + {t} × {}", c.steps);
        assert!(h <= VALUE_BUDGET as isize + t * c.steps, "{name}: {h} B");
        if name == "the end" {
            assert!(c.rebuilds > 0, "the budget was reached");
        }
    }
}

/// The find field's, with 256 B queries and its 16,384 B budget.
#[gpui_kit::test]
fn find_field_history_holds_its_budget(cx: &mut TestAppContext) {
    crate::heap::mine();
    let t = fixed_per_step(cx);
    let ops = churn_ops();
    let third = ops.iter().position(|o| matches!(o, Op::Paste(3))).expect("the 3rd");
    let twelfth = ops.iter().position(|o| matches!(o, Op::Paste(13))).expect("the 13th") - 1;
    for (name, upto) in [("3rd paste", third), ("12th's redos", twelfth), ("the end", ops.len() - 1)] {
        let pg = find_field(cx, "find-budget");
        let c = churn(cx, &pg, Field::Find, &ops[..=upto], FIND_CAP, FIND_BUDGET);
        let charged = charge(cx, &pg, Field::Find) as isize;
        let h = history(cx, &pg, Field::Find);
        eprintln!("FIND-HISTORY {name}: {h} B held, charge {charged}, {} steps, {} rebuilds", c.steps, c.rebuilds);
        assert!(h <= charged + t * c.steps, "{name}: {h} B > the charge {charged} + {t} × {}", c.steps);
        assert!(h <= FIND_BUDGET as isize + t * c.steps, "{name}: {h} B");
        if name == "the end" {
            assert!(c.rebuilds > 0, "the budget was reached");
        }
    }
}

/// A paste whose charge would pass the budget rebuilds first, so it stays
/// undoable: one undo restores the value from before it.
#[gpui_kit::test]
fn a_paste_at_the_budget_rebuilds_first_and_stays_undoable(cx: &mut TestAppContext) {
    let pg = url_field(cx, "rebuild-first", &nth(0, VALUE_CAP));
    for i in 1..=3 {
        paste(cx, pg.w, &nth(i, VALUE_CAP));
    }
    let was = input(cx, &pg, Field::Value).entity_id();
    assert!(charge(cx, &pg, Field::Value) + crate::settings_editor::EditGuard::charge_of(VALUE_CAP, VALUE_CAP, false) > VALUE_BUDGET);
    paste(cx, pg.w, &nth(4, VALUE_CAP));
    assert_ne!(input(cx, &pg, Field::Value).entity_id(), was, "rebuilt first");
    assert!(text(cx, &pg, Field::Value) == nth(4, VALUE_CAP), "then pasted");
    act(cx, pg.w, Undo);
    assert!(text(cx, &pg, Field::Value) == nth(3, VALUE_CAP), "one undo: the value before the paste");
}

/// How a composition ends ([`a_composition_holds_its_fields_budget`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum End {
    /// Committed, and the history held then measured.
    Commit,
    /// Committed, then one undo and one redo.
    Undone,
    /// Cancelled (an empty preedit), and the history measured.
    Cancel,
}

/// Shady #116 B1: an IME composition holds its field's budget. The kit
/// sends no edit event for a preedit but keeps each one in the
/// composition's transaction until it ends, and the commit's event says one
/// whole-text span. On each field, a composition over its whole text (at
/// the cap) of 100 distinct preedits at the cap, alternating ASCII and
/// three-byte characters, frames drawn between: after each, the charge
/// within the budget, the text the preedit, still marked; the field
/// rebuilt on the way. Then committed (or cancelled): what the history
/// holds within the charge plus the kit's fixed part a step, each preedit
/// a step; and one undo of the commit leaves what a cancel would (the
/// history before it was cleared, as its note says), one redo the commit.
#[gpui_kit::test]
fn a_composition_holds_its_fields_budget(cx: &mut TestAppContext) {
    crate::heap::mine();
    let t = fixed_per_step(cx);
    for (f, cap, budget) in [(Field::Value, VALUE_CAP, VALUE_BUDGET), (Field::Find, FIND_CAP, FIND_BUDGET)] {
        let preedit = |k: usize| if k.is_multiple_of(2) { nth(k, cap) } else { "あ".repeat(cap / 3) };
        for end in [End::Commit, End::Undone, End::Cancel] {
            let pg = match f {
                Field::Value => url_field(cx, "compose", &nth(0, cap)),
                Field::Find => {
                    let pg = find_field(cx, "compose");
                    paste(cx, pg.w, &nth(0, cap));
                    pg
                }
            };
            act(cx, pg.w, SelectAll);
            let (mut steps, mut rebuilds) = (0isize, 0);
            for k in 1..=100 {
                let was = input(cx, &pg, f).entity_id();
                mark(cx, &pg, f, &preedit(k));
                steps += 1;
                if input(cx, &pg, f).entity_id() != was {
                    rebuilds += 1;
                    steps = 1;
                }
                let c = charge(cx, &pg, f);
                assert!(c <= budget, "{f:?} {end:?}, preedit {k}: charge {c} > {budget}");
                assert!(text(cx, &pg, f) == preedit(k), "{f:?} {end:?}, preedit {k}: the text is the preedit");
                let n = preedit(k).encode_utf16().count();
                assert_eq!(marked(cx, &pg, f), Some(0..n), "{f:?} {end:?}, preedit {k}: still marked");
            }
            let fin = format!("{}!", nth(7, cap - 1));
            let was = input(cx, &pg, f).entity_id();
            match end {
                End::Commit | End::Undone => commit(cx, &pg, f, &fin),
                End::Cancel => mark(cx, &pg, f, ""),
            }
            let left = if end == End::Cancel { String::new() } else { fin.clone() };
            assert!(text(cx, &pg, f) == left && marked(cx, &pg, f).is_none(), "{f:?} {end:?}: ended");
            if input(cx, &pg, f).entity_id() != was {
                // The commit's own charge passed the budget: rebuilt after it.
                rebuilds += 1;
                steps = 0;
            }
            if end == End::Undone {
                // What a cancel leaves (the whole text was marked over); or,
                // the commit having rebuilt the field, nothing to undo.
                let undone = if steps == 0 { fin.clone() } else { String::new() };
                act(cx, pg.w, Undo);
                eprintln!("COMPOSITION {f:?} undone: the commit rebuilt the field: {}", steps == 0);
                assert!(text(cx, &pg, f) == undone, "{f:?}: one undo, what a cancel leaves, or nothing past a rebuild");
                act(cx, pg.w, Redo);
                assert!(text(cx, &pg, f) == fin, "{f:?}: one redo, the commit");
                assert!(rebuilds > 0, "{f:?} {end:?}: the budget was reached");
                continue;
            }
            let charged = charge(cx, &pg, f) as isize;
            let h = history(cx, &pg, f);
            eprintln!("COMPOSITION {f:?} {end:?}: {h} B held, charge {charged}, {steps} steps, {rebuilds} rebuilds");
            assert!(h <= charged + t * (steps + 1), "{f:?} {end:?}: {h} B > the charge {charged} + {t} × {}", steps + 1);
            assert!(rebuilds > 0, "{f:?} {end:?}: the budget was reached");
        }
    }
}

/// Shady #116 B2: a paste whose charge passes the budget rebuilds first and
/// is made on the new input: its caret, at the paste's end, is in the
/// field's view two frames on, as an ordinary paste's is. On each field,
/// from scroll zero (short pastes after long ones up to the budget), a
/// long paste: the text, the caret's byte range and one undo as before.
#[gpui_kit::test]
fn a_rebuild_first_paste_shows_its_caret(cx: &mut TestAppContext) {
    for (f, cap, budget) in [(Field::Value, VALUE_CAP, VALUE_BUDGET), (Field::Find, FIND_CAP, FIND_BUDGET)] {
        let pg = match f {
            Field::Value => url_field(cx, "reveal", "a"),
            Field::Find => {
                let pg = find_field(cx, "reveal");
                paste(cx, pg.w, "a");
                pg
            }
        };
        let short = ["a", "b"];
        let mut i = 0;
        while charge(cx, &pg, f) + EditGuard::charge_of(1, cap, false) <= budget {
            paste(cx, pg.w, &nth(i + 1, cap));
            paste(cx, pg.w, short[(i + 1) % 2]);
            i += 1;
        }
        let was = input(cx, &pg, f).entity_id();
        let before = text(cx, &pg, f);
        let zero = input(cx, &pg, f).read_with(cx, |s, _| s.scroll_offset());
        assert_eq!(zero.x, gpui_kit::px(0.), "{f:?}: from scroll zero");
        let long = nth(99, cap);
        paste(cx, pg.w, &long);
        settle(cx, pg.w, 2);
        assert_ne!(input(cx, &pg, f).entity_id(), was, "{f:?}: rebuilt first");
        assert!(text(cx, &pg, f) == long, "{f:?}: then pasted");
        let i = input(cx, &pg, f);
        assert_eq!(i.read_with(cx, |s, _| s.selected_range()), cap..cap, "{f:?}: the caret at its end");
        let g = i.read_with(cx, |s, _| s.prepaint_caret_geometry()).expect("a caret drawn");
        let (caret, view) = (g.cursor_bounds(), g.input_bounds());
        eprintln!("REVEAL {f:?}: caret {caret:?}, the field {view:?}");
        assert!(
            caret.left() >= view.left() && caret.right() <= view.right(),
            "{f:?}: the caret in the field's view: caret x {:?}..{:?}, the field {:?}..{:?}",
            caret.left(),
            caret.right(),
            view.left(),
            view.right()
        );
        act(cx, pg.w, Undo);
        assert_eq!(text(cx, &pg, f), before, "{f:?}: one undo, the value before it");
    }
}

/// The value field's close leaves no history: by commit, Esc, a blur and
/// the tab's close, after PLAN §4's churn, the old entity (and every input
/// the churn rebuilt) is released, and the account ends within 8 KiB of a
/// control run that opened and closed the field with no edits. (The plan
/// says 4 KiB. Two control runs of this harness, alike, differ by up to
/// 5.6 KiB, so the bound is 8 KiB, against the 32 KiB of history the churn
/// holds before the close.)
///
/// All runs share one window, after a churn and close that is not counted,
/// and each starts once the window's account holds steady ([`steady`]), so
/// what the window keeps of its draws is the same for both. A pair counts
/// once its control ends within 1 KiB of the attempt before's (what the
/// window keeps of its draws has stopped growing) and GPUI's frame profiler
/// held its size through it ([`profiler_size`]). At most 6 attempts. The field is the daemon label's (a commit
/// only saves it; the dash URL's also looks for the dash again), and the
/// churn ends with the value it opened on pasted back, so both runs commit
/// the same.
#[gpui_kit::test]
fn value_field_close_leaves_no_history(cx: &mut TestAppContext) {
    crate::heap::mine();
    #[derive(Clone, Copy, Debug)]
    enum Close {
        Commit,
        Esc,
        Blur,
        Tab,
    }
    let pg = page(cx, "close-history", NARROW);
    let opened = nth(0, VALUE_CAP);
    // What the account holds over its start: before the close, and after.
    let run = |cx: &mut TestAppContext, how: Close, edits: bool| -> (isize, isize) {
        if pg.view.read_with(cx, |v, _| v.model.tabs.view()) != Some(crate::tabs::Kind::Settings) {
            open_settings(cx, &pg);
        }
        pg.view.update(cx, |v, _| v.app_settings.daemon_label = Some(opened.clone()));
        steady(cx, &pg);
        let start = held(cx, &pg);
        find(cx, pg.w, "daemon_label");
        click(cx, pg.w, "field-daemon-label");
        assert_eq!(pg.view.read_with(cx, |v, _| v.settings.editing), Some(crate::settings_view::Edit::DaemonLabel), "open");
        let inputs = if edits {
            let c = churn(cx, &pg, Field::Value, &churn_ops(), VALUE_CAP, VALUE_BUDGET);
            paste(cx, pg.w, &opened);
            c.inputs
        } else {
            Vec::new()
        };
        assert!(text(cx, &pg, Field::Value) == opened, "{how:?}: the value it opened on");
        let mid = held(cx, &pg);
        let old = input(cx, &pg, Field::Value).downgrade();
        match how {
            Close::Commit => press(cx, pg.w, "enter"),
            Close::Esc => press(cx, pg.w, "escape"),
            Close::Blur => click(cx, pg.w, "settings-find"),
            Close::Tab => {
                pg.view.update(cx, |v, cx| {
                    let ix = v.model.tabs.iter().position(|t| t.kind == crate::tabs::Kind::Settings).expect("Settings");
                    v.close_tab(ix);
                    v.views_changed(cx);
                });
                settle(cx, pg.w, 8);
            }
        }
        // The find box as it was, so the next run starts the same.
        if pg.view.read_with(cx, |v, _| v.model.tabs.view()) == Some(crate::tabs::Kind::Settings) {
            find(cx, pg.w, "");
        }
        cx.run_until_parked();
        steady(cx, &pg);
        let end = held(cx, &pg);
        assert!(pg.view.read_with(cx, |v, _| v.settings.editing.is_none()), "{how:?}: closed");
        assert!(old.upgrade().is_none(), "{how:?}: the old entity is released");
        let kept = inputs.iter().filter(|i| i.upgrade().is_some()).count();
        assert_eq!(kept, 0, "{how:?}: of the churn's {} inputs, {kept} still held", inputs.len());
        assert_eq!(text(cx, &pg, Field::Value), "", "{how:?}: an empty one in its place");
        (mid - start, end - start)
    };
    run(cx, Close::Commit, true);
    for how in [Close::Commit, Close::Esc, Close::Blur, Close::Tab] {
        let mut before: Option<isize> = None;
        let (control, churned) = (0..6)
            .find_map(|attempt| {
                let p = profiler_size(cx, pg.w);
                let control = run(cx, how, false);
                let churned = run(cx, how, true);
                let grew = profiler_size(cx, pg.w) != p;
                let level = before.is_some_and(|b| (control.1 - b).abs() <= 1024);
                before = Some(control.1);
                eprintln!(
                    "CLOSE-HISTORY {how:?} attempt {attempt} (control {} B, level: {level}, the profiler grew: {grew}): open {} B over the control, closed {} B",
                    control.1,
                    churned.0 - control.0,
                    churned.1 - control.1
                );
                (level && !grew).then_some((control, churned))
            })
            .unwrap_or_else(|| panic!("{how:?}: the window's account never held level"));
        assert!(churned.0 - control.0 > 4096, "{how:?}: the churn held history to lose ({} B)", churned.0 - control.0);
        assert!((churned.1 - control.1).abs() <= 8192, "{how:?}: closed, {} B against a control's {} B", churned.1, control.1);
    }
}
