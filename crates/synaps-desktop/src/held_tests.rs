//! An answered card's room held at the list's end, as the app met it
//! (#23 r6): the card's row goes from the list in the frame its room
//! lands, in a splice that may hold the list's top row. GPUI moves that
//! top row to the splice's start, offset 0 (`ListState::splice_focusable`),
//! and the rows of the splice have no height until they are laid out
//! again. `ui_tests`' held-room tests have their top row above it, every
//! row measured; here the window is short enough that it is in it (the
//! bench's h5 film at 1920×1080: the column jumped 64 px down as the room
//! landed, and once the output had used the room up the list stayed 88 px
//! short of its end, the output streaming under the composer).
//!
//! The helpers are `ui_tests`' own, from [`super`] (`test_kit`).

use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext};
use synaps_client::protocol::{LlmEvent, SessionEvent, StreamEvent};

use super::{fluid_frame, motion_level, open_at, row_tops, settle};
use crate::app::AppView;

/// The `--offline` demo's window, `w`×`h` px at the default text step.
fn open_h(cx: &mut TestAppContext, h: f32) -> (AnyWindowHandle, Entity<AppView>, async_channel::Receiver<synaps_client::Event>) {
    open_at(cx, (1440., h), crate::theme::Panels::default().text_size)
}

/// The largest move of a row on screen between two of these.
fn moved(a: &std::collections::HashMap<(u64, bool), f32>, b: &std::collections::HashMap<(u64, bool), f32>) -> f32 {
    b.iter().filter_map(|(r, y)| a.get(r).map(|y0| (y - y0).abs())).fold(0f32, f32::max)
}

/// How far the list is short of its end (past its bottom padding).
fn short(cx: &mut TestAppContext, view: &Entity<AppView>) -> f32 {
    view.read_with(cx, |v, cx| {
        let pad = (28. * crate::theme::metrics(cx).r).round();
        f32::from(v.list.max_offset_for_scrollbar().y) + pad + f32::from(v.list.scroll_px_offset_for_scrollbar().y)
    })
}

/// The h5 flow (the bench's `SYNAPS_BENCH_HELD`): the demo's card answered
/// elsewhere while the list follows the tail, `extra` calls after its own
/// (the bench's card sits on its run's first call), its room held at the
/// end; the composer grows three lines (or not); a new call at the run's
/// end streams until the room is used up, and on. At these window heights
/// r5 (49a3c6a) strands the list: 6 calls at 560 px land the room at the
/// end, its base read before the splice took the card's row out above the
/// view (123 px short by line 30); 6 at 660 px and 10 at 800 px have the
/// list's top row in the splice (the column jumps 51 and 68 px down as the
/// room lands, short of its end from then on). The card as the list's
/// last row (no call after it) drew the room inside its own row: the
/// splice takes both out of GPUI's sums (a control: the room must not be
/// read as used up there). Every case: the column holds still as the room
/// lands (no row faster than a spring step), the list is at its end after
/// every line, and once the room is used up it follows.
#[gpui_kit::test]
fn a_card_room_landing_in_the_top_rows_splice_still_follows(cx: &mut TestAppContext) {
    for (extra, h, grow) in [(6usize, 560f32, true), (6, 660., false), (10, 800., true), (0, 600., true)] {
        let case = format!("{extra} calls after the card, {h} px{}", if grow { ", the composer three lines" } else { "" });
        let (w, view, _events) = open_h(cx, h);
        motion_level(cx, &view, crate::motion::Level::Full);
        view.update(cx, |v, _| {
            v.client = None;
            v.model.transcript.set_busy(true);
        });
        settle(cx, w, 8);
        let apply = |cx: &mut TestAppContext, e: LlmEvent| {
            view.update(cx, |v, cx| {
                v.model.transcript.apply(&SessionEvent::Stream { event: StreamEvent::Llm(e) });
                v.sync_list();
                cx.notify();
            });
            cx.run_until_parked();
        };
        for c in 0..extra {
            let id = format!("x{c}");
            apply(
                cx,
                LlmEvent::ToolUse {
                    tool_name: "bash".into(),
                    tool_id: id.clone(),
                    input: serde_json::json!({"command": format!("cargo check -p c{c}")}),
                },
            );
            apply(cx, LlmEvent::ToolResult { tool_id: id, result: "ok".into() });
        }
        for _ in 0..60 {
            fluid_frame(cx, w, &view);
        }
        assert!(view.read_with(cx, |v, _| v.list.is_following_tail()), "{case}: following before the answer");
        let ids: Vec<u64> = view.read_with(cx, |v, _| v.model.transcript.pending_prompts().iter().map(|p| p.id).collect());
        assert!(!ids.is_empty(), "{case}: the demo's card");
        view.update(cx, |v, cx| {
            for prompt_id in ids {
                v.model.transcript.apply(&SessionEvent::PromptResolved { prompt_id });
            }
            cx.notify();
        });
        // The card folds into its row, its room lands: a frame at a time.
        let mut last = row_tops(cx, &view);
        let mut landing = 0f32;
        for _ in 0..120 {
            fluid_frame(cx, w, &view);
            let now = row_tops(cx, &view);
            landing = landing.max(moved(&last, &now));
            last = now;
        }
        assert!(view.read_with(cx, |v, _| v.fold_slack.get().is_some()), "{case}: the room it left, held");
        let landed = short(cx, &view);
        if grow {
            cx.update_window(w, |_, window, cx| {
                view.update(cx, |v, cx| {
                    v.composer
                        .update(cx, |s, cx| s.set_value("steer: and the relay too\nkeep the old token\nthen run it again", window, cx))
                })
            })
            .expect("the window");
            for _ in 0..80 {
                fluid_frame(cx, w, &view);
            }
            assert!(view.read_with(cx, |v, _| v.fold_slack.get().is_some()), "{case}: held as the composer grew");
        }
        // A new call at the run's end: its output grows into the room,
        // then past it.
        apply(
            cx,
            LlmEvent::ToolUse {
                tool_name: "bash".into(),
                tool_id: "h0".into(),
                input: serde_json::json!({"command": "cargo test -p relay --test held"}),
            },
        );
        apply(cx, LlmEvent::ToolStarted { tool_id: "h0".into() });
        let (mut used, mut worst) = (None, (0f32, 0usize));
        for k in 0..30 {
            apply(cx, LlmEvent::ToolResultDelta { tool_id: "h0".into(), delta: format!("test held_{k} ... ok\n") });
            for _ in 0..10 {
                fluid_frame(cx, w, &view);
            }
            if used.is_none() && view.read_with(cx, |v, _| v.fold_slack.get().is_none()) {
                used = Some(k);
            }
            let s = short(cx, &view);
            if s > worst.0 {
                worst = (s, k);
            }
        }
        let (following, end) = view.read_with(cx, |v, _| (v.list.is_following_tail(), v.at_end()));
        eprintln!(
            "{case}: landing step {landing} px, {landed} px short as it landed; used up at line {used:?}; worst {} px short (line {}); following {following}, at end {end:?}",
            worst.0, worst.1
        );
        assert!(landing <= 17.5, "{case}: as the room landed a row moved {landing} px in a frame");
        assert!(landed <= crate::app::END_SLOP, "{case}: landed {landed} px short of its end");
        assert!(used.is_some(), "{case}: the output used the room up");
        assert!(worst.0 <= crate::app::END_SLOP, "{case}: line {}: {} px short of its end", worst.1, worst.0);
        assert!(following && end == Some(true), "{case}: it follows the tail ({following}, {end:?})");
    }
}

/// `e` as the daemon streams it to the attached session: its transcript,
/// its workers, the list synced.
fn apply(cx: &mut TestAppContext, view: &Entity<AppView>, e: SessionEvent) {
    view.update(cx, |v, cx| {
        v.model.stats.subagents.on_session(&e);
        v.model.transcript.apply(&e);
        v.sync_list();
        cx.notify();
    });
    cx.run_until_parked();
}

fn llm(e: LlmEvent) -> SessionEvent {
    SessionEvent::Stream { event: StreamEvent::Llm(e) }
}

/// Worker `n`'s row on the daemon's list (`sa_<n>`, `scout`), `status`.
fn worker_row(n: u64, status: &str) -> SessionEvent {
    let row = serde_json::json!({"subagent_id": n, "agent_name": "scout", "status": status, "cancel_requested": false,
        "elapsed_secs": 12.0, "finished_elapsed": if status == "running" { serde_json::Value::Null } else { serde_json::json!(12.0) },
        "step": "reading src/relay/session.rs", "tools": 3, "handle_id": format!("sa_{n}")});
    synaps_client::protocol::decode_event(&serde_json::json!({"ev": "subagent_rows", "rows": [row]})).expect("a rows event")
}

/// A worker's call `id` at the run's end, worker `n` running under it.
fn start_worker(cx: &mut TestAppContext, view: &Entity<AppView>, id: &str, n: u64) {
    let input = serde_json::json!({"agent": "scout", "task": "map the relay's callers"});
    apply(cx, view, llm(LlmEvent::ToolUse { tool_name: "subagent_start".into(), tool_id: id.into(), input }));
    let result = serde_json::json!({"handle_id": format!("sa_{n}"), "agent_name": "scout", "status": "running"}).to_string();
    apply(cx, view, llm(LlmEvent::ToolResult { tool_id: id.into(), result }));
    apply(cx, view, worker_row(n, "running"));
}

/// A finished bash call `id` at the run's end.
fn done_call(cx: &mut TestAppContext, view: &Entity<AppView>, id: &str) {
    let input = serde_json::json!({"command": format!("cargo check -p {id}")});
    apply(cx, view, llm(LlmEvent::ToolUse { tool_name: "bash".into(), tool_id: id.into(), input }));
    apply(cx, view, llm(LlmEvent::ToolResult { tool_id: id.into(), result: "ok".into() }));
}

/// Call `tool_id`'s row in the list.
fn row_of(cx: &mut TestAppContext, view: &Entity<AppView>, tool_id: &str) -> Option<usize> {
    view.read_with(cx, |v, _| {
        let item = v.model.shown().items().iter().find(|i| i.tool.as_ref().is_some_and(|t| t.tool_id == tool_id))?.id;
        v.disp.iter().position(|d| d.id == item && d.kind == crate::turns::Kind::Work)
    })
}

/// The list's top row and how far into it the view starts.
fn top(cx: &mut TestAppContext, view: &Entity<AppView>) -> (usize, f32) {
    view.read_with(cx, |v, _| {
        let t = v.list.logical_scroll_top();
        (t.item_ix, f32::from(t.offset_in_item))
    })
}

/// The h5 flow up to its room: the demo's card (on `toolu_d5`) answered
/// elsewhere while the list follows the tail, a worker's call after it
/// (`worker`: `scout`, running) and `extra` finished calls after that, at
/// `h` px. The card folds into its row and its room lands at the end, held
/// (the list not following). `touch`: the user scrolls 3 px up as it folds,
/// so the list is paused when it lands. The largest move of a row in a
/// frame meanwhile, but for that scroll's own.
fn land(
    cx: &mut TestAppContext,
    h: f32,
    worker: bool,
    extra: usize,
    touch: bool,
) -> (AnyWindowHandle, Entity<AppView>, async_channel::Receiver<synaps_client::Event>, f32) {
    let (w, view, events) = open_h(cx, h);
    motion_level(cx, &view, crate::motion::Level::Full);
    view.update(cx, |v, _| {
        v.client = None;
        v.model.transcript.set_busy(true);
    });
    settle(cx, w, 8);
    if worker {
        start_worker(cx, &view, "w0", 41);
    }
    for c in 0..extra {
        done_call(cx, &view, &format!("x{c}"));
    }
    for _ in 0..60 {
        fluid_frame(cx, w, &view);
    }
    assert!(view.read_with(cx, |v, _| v.list.is_following_tail()), "following before the answer");
    let ids: Vec<u64> = view.read_with(cx, |v, _| v.model.transcript.pending_prompts().iter().map(|p| p.id).collect());
    assert!(!ids.is_empty(), "the demo's card");
    view.update(cx, |v, cx| {
        for prompt_id in ids {
            v.model.transcript.apply(&SessionEvent::PromptResolved { prompt_id });
        }
        cx.notify();
    });
    let mut last = row_tops(cx, &view);
    let mut landing = 0f32;
    for f in 0..120 {
        if touch && f == 4 {
            view.update(cx, |v, cx| {
                // In place of the wheel (its capture owner touches the scroller).
                v.scroller.touch();
                v.list.scroll_by(gpui_kit::px(-3.));
                cx.notify();
            });
        }
        fluid_frame(cx, w, &view);
        let now = row_tops(cx, &view);
        landing = landing.max(moved(&last, &now) - if touch && f == 4 { 3. } else { 0. });
        last = now;
    }
    (w, view, events, landing)
}

fn held(cx: &mut TestAppContext, view: &Entity<AppView>) -> bool {
    view.read_with(cx, |v, _| v.fold_slack.get().is_some())
}

/// Row `ix`'s top on screen.
fn y_of(cx: &mut TestAppContext, view: &Entity<AppView>, ix: usize) -> f32 {
    view.read_with(cx, |v, _| v.list.bounds_for_item(ix).map_or(f32::NAN, |b| f32::from(b.top())))
}

/// What a stream under a held room did ([`stream_lines`]).
#[derive(Debug)]
struct Lines {
    /// The line that used the room up.
    used: Option<usize>,
    /// From that line on: how far short of its end the list was at worst
    /// (and after which line), and the first line after which it did not
    /// follow the tail.
    worst: (f32, usize),
    unfollowed: Option<usize>,
    /// The largest drop of the view's top row in a frame (the row at the
    /// top the frame before, by identity: down is positive).
    drop: f32,
}

/// `n` lines streamed into call `id`, ten frames each, `each(k)` first
/// (the storm's other events).
fn stream_lines(
    cx: &mut TestAppContext,
    w: AnyWindowHandle,
    view: &Entity<AppView>,
    id: &str,
    n: usize,
    each: &mut dyn FnMut(&mut TestAppContext, usize),
) -> Lines {
    let mut out = Lines { used: None, worst: (0., 0), unfollowed: None, drop: 0. };
    let mut last = row_tops(cx, view);
    for k in 0..n {
        each(cx, k);
        apply(cx, view, llm(LlmEvent::ToolResultDelta { tool_id: id.into(), delta: format!("test held_{k} ... ok\n") }));
        for _ in 0..10 {
            fluid_frame(cx, w, view);
            let now = row_tops(cx, view);
            if let Some((top, y0)) = last.iter().min_by(|a, b| a.1.total_cmp(b.1))
                && let Some(y1) = now.get(top)
            {
                out.drop = out.drop.max(y1 - y0);
            }
            last = now;
        }
        if out.used.is_none() && !held(cx, view) {
            out.used = Some(k);
        }
        if out.used.is_some() {
            let s = short(cx, view);
            if s > out.worst.0 {
                out.worst = (s, k);
            }
            if out.unfollowed.is_none() && !view.read_with(cx, |v, _| v.list.is_following_tail()) {
                out.unfollowed = Some(k);
            }
        }
    }
    out
}

/// The composer three lines (a steer typed as the turn runs).
fn grow_composer(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    cx.update_window(w, |_, window, cx| {
        view.update(cx, |v, cx| {
            v.composer.update(cx, |s, cx| s.set_value("steer: and the relay too\nkeep the old token\nthen run it again", window, cx))
        })
    })
    .expect("the window");
}

/// Shady #23 r6 H1 (a): the answered call runs as the daemon runs it, its
/// row the list's top row (a short window: the run's calls after the card
/// above the view's top). Every event of it (`ToolStarted`, each delta)
/// splices its row, and GPUI put the list's top at that row's start: the
/// column dropped by however far into it the view was (47 and 59 px), the
/// list that far short of its end for good, the output streaming under the
/// composer. The row holds still while its room is held; the output takes
/// the room, and the list follows from there.
#[gpui_kit::test]
fn the_answered_calls_own_output_keeps_the_held_column_put(cx: &mut TestAppContext) {
    answered_streams(cx);
}

fn answered_streams(cx: &mut TestAppContext) {
    for (extra, h) in [(0usize, 440f32), (2, 520.)] {
        let case = format!("{extra} calls after the card, {h} px");
        let (w, view, _events, landing) = land(cx, h, false, extra, false);
        assert!(held(cx, &view), "{case}: the room it left, held");
        let d5 = row_of(cx, &view, "toolu_d5").expect("the answered call's row");
        let (ix, off) = top(cx, &view);
        assert!(ix == d5 && off >= 8., "{case}: the answered call's row is the list's top row, into it ({ix} +{off})");
        let y0 = y_of(cx, &view, d5);
        apply(cx, &view, llm(LlmEvent::ToolStarted { tool_id: "toolu_d5".into() }));
        let (mut drop, mut used, mut worst) = (0f32, None, (0f32, 0usize));
        for k in 0..30 {
            apply(cx, &view, llm(LlmEvent::ToolResultDelta { tool_id: "toolu_d5".into(), delta: format!("test race_{k} ... ok\n") }));
            for _ in 0..10 {
                fluid_frame(cx, w, &view);
                if held(cx, &view) {
                    drop = drop.max((y_of(cx, &view, d5) - y0).abs());
                }
            }
            if used.is_none() && !held(cx, &view) {
                used = Some(k);
            }
            let s = short(cx, &view);
            if s > worst.0 {
                worst = (s, k);
            }
        }
        apply(cx, &view, llm(LlmEvent::ToolResult { tool_id: "toolu_d5".into(), result: "test result: ok. 30 passed".into() }));
        for _ in 0..120 {
            fluid_frame(cx, w, &view);
        }
        let follows = view.read_with(cx, |v, _| v.list.is_following_tail() && v.at_end() == Some(true));
        eprintln!("{case}: top +{off}; landing {landing}; row moved {drop} px held; used at {used:?}; worst {worst:?}; follows {follows}");
        assert!(landing <= 17.5, "{case}: as the room landed a row moved {landing} px in a frame");
        assert!(drop <= 0.5, "{case}: its room held, the answered call's row moved {drop} px");
        assert!(used.is_some(), "{case}: the output used the room up");
        assert!(worst.0 <= crate::app::END_SLOP, "{case}: line {}: {} px short of its end", worst.1, worst.0);
        assert!(follows, "{case}: it follows the tail at its end");
    }
}

/// Shady #23 r6 H1 (b): a worker's call row is the list's top row while a
/// room is held, and its block changes shape (the worker failed): its row
/// is spliced again (`turns::rev` holds the block's shape), and the column
/// dropped as in (a), the list short of its end for good. Its row may move
/// (its block is shorter), never more than a spring step in a frame; a new
/// call's output then takes the room, and once it is used up the list is
/// at its end and follows (#23 r9: the room's sums still read it short
/// there, by ~100 px; with no scroll of the user's, it lands at its true end).
#[gpui_kit::test]
fn a_workers_row_at_the_top_changing_keeps_the_held_column_put(cx: &mut TestAppContext) {
    for (extra, h) in [(0usize, 440f32)] {
        let case = format!("a worker and {extra} calls after the card, {h} px");
        let (w, view, _events, _) = land(cx, h, true, extra, false);
        assert!(held(cx, &view), "{case}: the room it left, held");
        let w0 = row_of(cx, &view, "w0").expect("the worker's row");
        let (ix, off) = top(cx, &view);
        assert!(ix == w0 && off >= 8., "{case}: the worker's row is the list's top row, into it ({ix} +{off})");
        let input = serde_json::json!({"command": "cargo test -p relay --test held"});
        apply(cx, &view, llm(LlmEvent::ToolUse { tool_name: "bash".into(), tool_id: "h0".into(), input }));
        apply(cx, &view, llm(LlmEvent::ToolStarted { tool_id: "h0".into() }));
        let run = stream_lines(cx, w, &view, "h0", 30, &mut |cx, k| {
            if k == 0 {
                apply(cx, &view, worker_row(41, "failed"));
            }
        });
        let follows = view.read_with(cx, |v, _| v.list.is_following_tail() && v.at_end() == Some(true));
        eprintln!("{case}: top +{off}; {run:?}; follows {follows}");
        assert!(run.drop <= 17.5, "{case}: the view's top row dropped {} px in a frame", run.drop);
        assert!(run.used.is_some(), "{case}: the output used the room up");
        assert!(run.worst.0 <= crate::app::END_SLOP, "{case}: line {}: {} px short of its end", run.worst.1, run.worst.0);
        assert!(run.unfollowed.is_none() && follows, "{case}: it follows the tail from use-up on ({:?})", run.unfollowed);
    }
}

/// Shady #23 r6 H1 (c), main's own: a reader scrolled back, a worker's row
/// their top row, 12 px into it. Its block changes shape and its row is
/// spliced again: GPUI put the list's top at its start, the reader's rows
/// jumping 12 px down. Nothing moves.
#[gpui_kit::test]
fn a_reader_scrolled_back_keeps_a_changing_top_row_put(cx: &mut TestAppContext) {
    reader_back(cx);
}

fn reader_back(cx: &mut TestAppContext) {
    let (w, view, _events) = open_h(cx, 700.);
    motion_level(cx, &view, crate::motion::Level::Full);
    view.update(cx, |v, _| {
        v.client = None;
        v.model.transcript.set_busy(true);
    });
    settle(cx, w, 8);
    start_worker(cx, &view, "w0", 41);
    for c in 0..24 {
        done_call(cx, &view, &format!("x{c}"));
    }
    for _ in 0..60 {
        fluid_frame(cx, w, &view);
    }
    let w0 = row_of(cx, &view, "w0").expect("the worker's row");
    view.update(cx, |v, cx| {
        v.scroller.touch();
        v.list.scroll_to(gpui_kit::ListOffset { item_ix: w0, offset_in_item: gpui_kit::px(12.) });
        cx.notify();
    });
    for _ in 0..20 {
        fluid_frame(cx, w, &view);
    }
    let (ix, off) = top(cx, &view);
    assert!(ix == w0 && (off - 12.).abs() < 0.5, "scrolled back to the worker's row, 12 px in ({ix} +{off})");
    let before = row_tops(cx, &view);
    let y0 = y_of(cx, &view, w0);
    apply(cx, &view, worker_row(41, "failed"));
    for _ in 0..30 {
        fluid_frame(cx, w, &view);
    }
    let (y1, after) = (y_of(cx, &view, w0), row_tops(cx, &view));
    let above = before.iter().filter(|(_, y)| **y <= y0).filter_map(|(r, y)| after.get(r).map(|y1| (y1 - y).abs())).fold(0f32, f32::max);
    eprintln!("reader back: the worker's row {y0} -> {y1}; rows at or above it moved {above} px");
    assert!((y1 - y0).abs() <= 0.5 && above <= 0.5, "the reader's top row moved: {y0} -> {y1} ({above} px)");
    assert!(!view.read_with(cx, |v, _| v.list.is_following_tail()), "still scrolled back");
}

/// Shady #23 r6 Q1: the list already paused as the room lands (the user
/// scrolled as the card folded), the landing splice holding its top row
/// (r6's 660 and 800 px cases): `Slack::fresh` cannot keep it following,
/// and GPUI put its top at the splice's start (a 48 px jump). The rows hold
/// (the room may go then: left, out of sight, the list back at its end).
#[gpui_kit::test]
fn a_room_landing_on_a_paused_list_keeps_its_rows_put(cx: &mut TestAppContext) {
    paused_landing(cx);
}

fn paused_landing(cx: &mut TestAppContext) {
    for (extra, h) in [(6usize, 660f32)] {
        let case = format!("{extra} calls after the card, {h} px, scrolled 3 px as it folded");
        let (_w, _view, _events, landing) = land(cx, h, false, extra, true);
        eprintln!("{case}: landing {landing} px");
        assert!(landing <= 17.5, "{case}: as the room landed a row moved {landing} px in a frame");
    }
}

/// #23 r9, the hold's safety net: a storm of splices under a held room at
/// 440, 660 and 800 px. The answered call starts and streams into its own
/// row, its run's worker fails, another worker starts and fails, the
/// composer grows three lines. Whatever the room's sums make of that, once
/// it is used up the list is at its end, and it follows every line after.
#[gpui_kit::test]
fn a_splice_storm_under_a_held_room_ends_at_its_end_and_follows(cx: &mut TestAppContext) {
    for h in [440f32, 660., 800.] {
        let case = format!("{h} px");
        let (w, view, _events, _) = land(cx, h, true, 0, false);
        assert!(held(cx, &view), "{case}: the room it left, held");
        apply(cx, &view, llm(LlmEvent::ToolStarted { tool_id: "toolu_d5".into() }));
        let run = stream_lines(cx, w, &view, "toolu_d5", 40, &mut |cx, k| match k {
            2 => apply(cx, &view, worker_row(41, "failed")),
            4 => grow_composer(cx, w, &view),
            6 => start_worker(cx, &view, "w1", 42),
            9 => apply(cx, &view, worker_row(42, "failed")),
            _ => {}
        });
        let follows = view.read_with(cx, |v, _| v.list.is_following_tail() && v.at_end() == Some(true));
        eprintln!("storm, {case}: {run:?}; follows {follows}");
        assert!(run.used.is_some(), "{case}: the output used the room up");
        assert!(run.worst.0 <= crate::app::END_SLOP, "{case}: line {}: {} px short of its end", run.worst.1, run.worst.0);
        assert!(run.unfollowed.is_none() && follows, "{case}: it follows the tail from use-up on ({:?})", run.unfollowed);
    }
}
