//! The labels' move between their line over the row and the gutter with a
//! side panel's flight ([`crate::turns::HangFx`]; Haseeb, 2026-11-21: the
//! cut a frame after the rail landed read as choppy). The `--offline`
//! demo at 1100 px: with one side panel open the pane is under the hang
//! width, with both folded over it, and the column is capped either way
//! (a flight that also crosses the cap rewraps the column as it lands,
//! fluid.md §3.8, as before).
//!
//! The helpers are `ui_tests`' own, from [`super`] (`test_kit`).

use gpui_kit::{AnyWindowHandle, Entity, TestAppContext, VisualTestContext, px, size};
use synaps_client::protocol::{LlmEvent, SessionEvent, StreamEvent};

use super::{fluid_frame, motion_level, open_at, row_tops, settle, wait};
use crate::app::AppView;
use crate::motion::Level;

/// A selector's bounds as drawn: left, top, width, height.
fn bounds(cx: &mut TestAppContext, w: AnyWindowHandle, sel: String) -> Option<[f32; 4]> {
    let b = VisualTestContext::from_window(w, cx).debug_bounds(sel.leak())?;
    Some([f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height)])
}

/// The centre pane as drawn, the hang laid out and the hang drawn.
fn pane(cx: &mut TestAppContext, view: &Entity<AppView>) -> (f32, bool, f32) {
    view.read_with(cx, |v, _| {
        let p = v.width - crate::chrome::MARK - v.panel_fx.sides[0].shown - v.panel_fx.sides[1].shown;
        (p, v.layout.hang, v.layout.hang_k)
    })
}

/// Frames until nothing moves (at most 400): each checked by `each`.
fn fly(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, mut each: impl FnMut(&mut TestAppContext)) -> usize {
    let mut n = 0;
    while (view.read_with(cx, |v, _| v.fx.pending().0) || view.read_with(cx, |v, _| v.panel_fx.moving())) && n < 400 {
        fluid_frame(cx, w, view);
        each(cx);
        n += 1;
    }
    n
}

/// The demo at 1100 px with only `rail` (else the right panel) open.
fn open_one(cx: &mut TestAppContext, rail: bool) -> (AnyWindowHandle, Entity<AppView>) {
    let (w, view, _events) = open_at(cx, (1100., 800.), crate::theme::Panels::default().text_size);
    motion_level(cx, &view, Level::Full);
    view.update(cx, |v, cx| {
        v.panels.rail_collapsed = !rail;
        v.panels.right_collapsed = rail;
        // Short turns after the demo's: a label every few lines.
        for n in 0..8 {
            let t = &mut v.model.transcript;
            let user_text = Some(format!("Question {n}: where does the refresh race start?"));
            t.apply(&SessionEvent::TurnStarted { turn_baseline: 12 + 2 * n, trigger: "user".into(), user_text });
            let text = format!("Answer {n}: in the relay's token cache, two lines down from the lock.");
            t.apply(&SessionEvent::Stream { event: StreamEvent::Llm(LlmEvent::Text { text }) });
            t.apply(&SessionEvent::Idle);
        }
        v.sync_list();
        cx.notify();
    });
    // Their arrivals over.
    wait(cx, w, &view, 3000);
    (w, view)
}

/// The first labelled row whole in the view at or under the list's top
/// (not the top row: its label may be pinned).
fn labelled(cx: &mut TestAppContext, view: &Entity<AppView>) -> u64 {
    view.read_with(cx, |v, _| {
        let vb = v.list.viewport_bounds();
        let top = v.list.logical_scroll_top().item_ix;
        (top + 1..v.disp.len())
            .find(|&ix| {
                v.disp[ix].who != crate::turns::Who::None
                    && v.list.bounds_for_item(ix).is_some_and(|b| b.top() > vb.top() + px(60.) && b.top() < vb.bottom() - px(200.))
            })
            .map(|ix| v.disp[ix].id)
            .unwrap_or_else(|| {
                let rows: Vec<_> = (0..v.disp.len())
                    .map(|ix| {
                        (
                            ix,
                            v.disp[ix].who != crate::turns::Who::None,
                            v.list.bounds_for_item(ix).map(|b| (f32::from(b.top()), f32::from(b.size.height))),
                        )
                    })
                    .collect();
                panic!("a labelled row in the view: top {top} view {vb:?} {rows:?}")
            })
    })
}

/// How far a label may move in a frame, in the pane's steps (and a pixel's
/// rounding either way): its sideways move leads the pane's (the finish
/// gate of #33, S1: `turns::lead`), up to about three and a half times the
/// line's rate mid-way, the line here under a panel's width over the label's
/// way. Never a move on a still pane.
const LEAD: f32 = 2.;

/// The pinned label's id, if one is ([`AppView::sticky_label`]).
fn pinned(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) -> Option<u64> {
    let ids: Vec<u64> = view.read_with(cx, |v, _| v.disp.iter().filter(|d| d.who != crate::turns::Who::None).map(|d| d.id).collect());
    ids.into_iter().find(|id| bounds(cx, w, format!("sticky-label {id}")).is_some())
}

/// Every label drawn on its row clear of that row's card or block (the
/// finish gate of #33, S1: on a straight path its corner crossed it).
fn labels_clear(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, case: &str) -> usize {
    let ids: Vec<u64> = view.read_with(cx, |v, _| v.disp.iter().filter(|d| d.who != crate::turns::Who::None).map(|d| d.id).collect());
    let (line, size) = view.read_with(cx, |_, cx| {
        let m = crate::theme::metrics(cx);
        (m.body_line, m.label)
    });
    let mut checked = 0;
    for id in ids {
        // Its text's capitals (the labels are capitals: its em box, the
        // line box less its leading, less the descent).
        let Some(mut l) = bounds(cx, w, format!("label {id}")) else { continue };
        (l[1], l[3]) = (l[1] + (line - size) / 2., 0.8 * size);
        for sel in [format!("user-block {id}"), format!("card-slice {id}")] {
            if let Some(c) = bounds(cx, w, sel.clone()) {
                checked += 1;
                let apart =
                    l[0] + l[2] <= c[0] + 0.5 || c[0] + c[2] <= l[0] + 0.5 || l[1] + l[3] <= c[1] + 0.5 || c[1] + c[3] <= l[1] + 0.5;
                assert!(apart, "{case}: label {id} {l:?} over its {sel} {c:?}");
            }
        }
    }
    checked
}

/// Haseeb's report: closing the rail (or the right panel, Ctrl+B) across
/// the hang width moves the labels with the panel, not in a cut after it;
/// opening, back. The hang is the flight's end's from its first frame; the
/// label and its time travel between their places as the pane goes from
/// its width to the new one, and the label line's room closes (opens) with
/// them: no frame moves a label by more than the panel's own step (the
/// column moves half of it), the rows wrap as they did. Scrolled back, the
/// top row holds still (0.5 px); following, the list stays at its end.
/// Landed, the layout is a fresh one's at the new width, and no frame is
/// asked for.
#[gpui_kit::test]
fn labels_glide_with_a_panel_across_the_hang(cx: &mut TestAppContext) {
    for rail in [true, false] {
        let (w, view) = open_one(cx, rail);
        let (p0, hang0, _) = pane(cx, &view);
        let column = view.read_with(cx, |v, _| v.layout.column);
        assert!(!hang0, "{rail}: {p0} px with the panel open: inline");
        for follow in [false, true] {
            view.update(cx, |v, cx| {
                if follow {
                    v.list.scroll_to_end();
                } else {
                    let ix = v.disp.len() - 12;
                    v.list.scroll_to(gpui_kit::ListOffset { item_ix: ix, offset_in_item: px(13.) });
                }
                cx.notify();
            });
            // What it brought in has arrived (rows are born when first drawn).
            wait(cx, w, &view, 2000);
            fly(cx, w, &view, |_| {});
            for close in [true, false] {
                let case = format!("rail {rail} follow {follow} close {close}");
                let anchor = view.read_with(cx, |v, _| v.list.logical_scroll_top().item_ix);
                let top = view.read_with(cx, |v, _| v.list.bounds_for_item(anchor).map(|b| f32::from(b.top())));
                let id = labelled(cx, &view);
                let sel = format!("label {id}");
                // The label, and its row's top (the glide is the label's
                // move on its row; the row's own is the rows closing up).
                let at = |cx: &mut TestAppContext| {
                    let row = view.read_with(cx, |v, _| {
                        let ix = v.disp.iter().position(|d| d.id == id).expect("the row");
                        v.list.bounds_for_item(ix).map(|b| f32::from(b.top())).expect("the row drawn")
                    });
                    (bounds(cx, w, sel.clone()).expect("the label in every frame"), row)
                };
                let line = view.read_with(cx, |_, cx| {
                    let m = crate::theme::metrics(cx);
                    m.body_line.max(m.mono_line) + (2. * m.r).round()
                });
                let mut last = (pane(cx, &view).0, at(cx), row_tops(cx, &view), if close { 0. } else { 1. });
                // Following, the top message's label is pinned (Q7): it
                // moves no more than the pane each frame too, the toggle's
                // own frame first (the finish gate of #33, M1: opening, it
                // hopped up `pad_y` there, the pane still).
                let stuck = pinned(cx, w, &view);
                assert!(!follow || stuck.is_some(), "{case}: a label pinned");
                // Until its row's own line comes into the view, or the next
                // message's takes its place (the rows closing up, opening, at
                // the end move them): then that one, from where it is.
                let stuck = std::cell::Cell::new(stuck.map(|_| pinned(cx, w, &view).unwrap_or(0)));
                let was = std::cell::Cell::new(stuck.get().and_then(|id| bounds(cx, w, format!("sticky-label {id}"))));
                let tracked = std::cell::Cell::new(0);
                let pin = |cx: &mut TestAppContext, step: f32, k: f32| {
                    if let (Some(id), Some(b)) = (stuck.get(), was.get()) {
                        was.set(bounds(cx, w, format!("sticky-label {id}")));
                        let Some(now) = was.get() else {
                            stuck.set(pinned(cx, w, &view));
                            was.set(stuck.get().and_then(|id| bounds(cx, w, format!("sticky-label {id}"))));
                            return;
                        };
                        let moved = (now[0] - b[0]).hypot(now[1] - b[1]);
                        assert!(
                            moved <= LEAD * step + std::f32::consts::SQRT_2 + 1e-3,
                            "{case}: the pinned label moved {moved} px in a frame the pane moved {step} ({k}): {b:?} -> {now:?}"
                        );
                        tracked.set(tracked.get() + 1);
                    }
                };
                view.update(cx, |v, cx| v.toggle_panel(rail, cx));
                cx.run_until_parked();
                let (p, _, k) = pane(cx, &view);
                pin(cx, (p - last.0).abs(), k);
                assert!(stuck.get().is_none() || was.get().is_some(), "{case}: still pinned in the toggle's frame");
                let (mut between, mut worst, mut clear) = (0, 0f32, 0);
                let frames = fly(cx, w, &view, |cx| {
                    let (p, hang, k) = pane(cx, &view);
                    assert_eq!(hang, close, "{case}: the hang is the flight's end's from its start");
                    if k > 0. && k < 1. {
                        between += 1;
                    }
                    let (b, row) = at(cx);
                    let (lb, lrow) = last.1;
                    let step = (p - last.0).abs();
                    pin(cx, step, k);
                    clear += labels_clear(cx, w, &view, &case);
                    // On its row: a pixel's rounding either way.
                    let glide = (b[0] - lb[0]).hypot((b[1] - row) - (lb[1] - lrow));
                    worst = worst.max(glide - step);
                    assert!(
                        glide <= LEAD * step + std::f32::consts::SQRT_2 + 1e-3,
                        "{case}: the label moved {glide} px on its row in a frame the pane moved {step} ({k})"
                    );
                    // The rows close up (open) by the label lines' room
                    // this frame (on whole pixels, rounded once), nothing more: no row jumps.
                    let tops = row_tops(cx, &view);
                    let room = tops.len() as f32 * line * (k - last.3).abs() + 2.;
                    for (r, y) in &tops {
                        if let Some(y0) = last.2.get(r) {
                            assert!((y - y0).abs() <= room, "{case}: row {r:?} moved {y0} -> {y} ({k}, room {room})");
                        }
                    }
                    if follow {
                        let (following, end) = view.read_with(cx, |v, _| (v.list.is_following_tail(), v.list.is_scrolled_to_end()));
                        assert!(following && end != Some(false), "{case}: following, at the end ({end:?})");
                    } else {
                        // The label's own move too: the rows above it close
                        // a little, the column moves half the pane's step.
                        let moved = (b[0] - lb[0]).hypot(b[1] - lb[1]);
                        assert!(
                            moved <= LEAD * step + std::f32::consts::SQRT_2 + 1e-3,
                            "{case}: the label moved {moved} px in a frame the pane moved {step} ({k})"
                        );
                        let now = view.read_with(cx, |v, _| v.list.bounds_for_item(anchor).map(|b| f32::from(b.top())));
                        let (a, b) = (top.expect("the top row"), now.expect("the top row drawn"));
                        assert!((a - b).abs() <= 0.5, "{case}: the top row moved {a} -> {b} ({k})");
                    }
                    last = (p, (b, row), tops, k);
                });
                assert!(
                    frames > 30 && between > 30,
                    "{case}: it flew ({frames} frames, {between} between, worst step past the pane's {worst})"
                );
                assert!(clear > frames, "{case}: labels checked against their cards ({clear})");
                assert!(!follow || tracked.get() > 10, "{case}: the pinned label followed ({})", tracked.get());
                let (_, hang, k) = pane(cx, &view);
                assert_eq!((hang, k), (close, if close { 1. } else { 0. }), "{case}: landed");
                assert_eq!(view.read_with(cx, |v, _| v.layout.column), column, "{case}: the column capped either way");
                assert!(settle(cx, w, 8) <= 1, "{case}: no frame at rest");
                assert!(!view.read_with(cx, |v, _| v.fx.pending().0), "{case}: nothing pending");
                // A fresh layout at this width: every row measured again.
                let (tops, label) = (row_tops(cx, &view), bounds(cx, w, sel.clone()));
                view.update(cx, |v, cx| {
                    v.list.remeasure();
                    cx.notify();
                });
                settle(cx, w, 8);
                assert_eq!(row_tops(cx, &view), tops, "{case}: the rows as a fresh layout has them");
                assert_eq!(bounds(cx, w, sel.clone()), label, "{case}: the label where a fresh layout puts it");
                let rest = if close { format!("hang-label-{id}") } else { format!("label {id}") };
                assert!(bounds(cx, w, rest).is_some(), "{case}: drawn at rest");
            }
        }
    }
}

/// Reduced and Off: the panels move at once, and so do the labels, in the
/// frame (a cut); no frame after it.
#[gpui_kit::test]
fn labels_cut_at_reduced_and_off(cx: &mut TestAppContext) {
    let (w, view) = open_one(cx, true);
    for level in [Level::Reduced, Level::Off] {
        motion_level(cx, &view, level);
        settle(cx, w, 8);
        for close in [true, false] {
            view.update(cx, |v, cx| v.toggle_panel(true, cx));
            cx.run_until_parked();
            let (_, hang, k) = pane(cx, &view);
            assert_eq!((hang, k), (close, if close { 1. } else { 0. }), "{level:?} {close}: in the frame");
            assert!(settle(cx, w, 8) <= 1, "{level:?} {close}: no frame after it");
            assert!(!view.read_with(cx, |v, _| v.fx.pending().0), "{level:?}");
        }
    }
}

/// A window resize across the hang width (the window manager's, live):
/// a cut in its frame, the hang always the pane's, never a frame between,
/// and it stays as it is in the frames after (no flicker).
#[gpui_kit::test]
fn a_resize_across_the_hang_cuts(cx: &mut TestAppContext) {
    let (w, view) = open_one(cx, true);
    view.update(cx, |v, cx| {
        v.panels.rail_collapsed = true;
        cx.notify();
    });
    settle(cx, w, 8);
    let mut vcx = VisualTestContext::from_window(w, cx);
    for width in [1100., 940., 936., 930., 900., 960., 1100.] {
        vcx.simulate_resize(size(px(width), px(800.)));
        vcx.run_until_parked();
        let (p, hang, k) = view.read_with(&vcx, |v, _| {
            let p = v.width - crate::chrome::MARK - v.panel_fx.sides[0].shown - v.panel_fx.sides[1].shown;
            (p, v.layout.hang, v.layout.hang_k)
        });
        let m = view.read_with(&vcx, |_, cx| crate::theme::metrics(cx));
        assert_eq!(hang, crate::turns::hangs(p, &m), "{width}: {p} px");
        assert_eq!(k, if hang { 1. } else { 0. }, "{width}: drawn as laid out");
        for _ in 0..4 {
            vcx.update(|window, cx| window.simulate_next_frame(cx));
            vcx.run_until_parked();
            let now = view.read_with(&vcx, |v, _| (v.layout.hang, v.layout.hang_k));
            assert_eq!(now, (hang, k), "{width}: as it was");
        }
    }
}

/// Scrolled back in the demo at 1100 px (the rail open, inline), arrived.
fn scrolled_back(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<AppView>) {
    let (w, view) = open_one(cx, true);
    view.update(cx, |v, cx| {
        let ix = v.disp.len() - 12;
        v.list.scroll_to(gpui_kit::ListOffset { item_ix: ix, offset_in_item: px(13.) });
        cx.notify();
    });
    wait(cx, w, &view, 2000);
    fly(cx, w, &view, |_| {});
    (w, view)
}

/// A flight turned back half-way (Shady #33 M1, L1): the rail closes, and
/// ten frames on opens again. The hang is the new end's from the frame it
/// turns; the label turns from where it is (no frame moves it more than
/// the pane's step) and goes back inline; the top row holds; landed, the
/// rows are measured again (`HangFx` says so on that frame) and are a fresh
/// layout's; no frame at rest.
#[gpui_kit::test]
fn a_reversed_flight_turns_from_where_the_labels_are(cx: &mut TestAppContext) {
    let (w, view) = scrolled_back(cx);
    let anchor = view.read_with(cx, |v, _| v.list.logical_scroll_top().item_ix);
    let top = view.read_with(cx, |v, _| v.list.bounds_for_item(anchor).map(|b| f32::from(b.top()))).expect("the top row");
    let id = labelled(cx, &view);
    let sel = format!("label {id}");
    let (p0, ..) = pane(cx, &view);
    let wide = view.read_with(cx, |v, _| v.width - crate::chrome::MARK);
    view.update(cx, |v, cx| v.toggle_panel(true, cx));
    cx.run_until_parked();
    let mut last = (p0, bounds(cx, w, sel.clone()).expect("the label"), 0f32);
    let (mut frames, mut peak) = (0, 0f32);
    while frames < 400 && (frames < 12 || view.read_with(cx, |v, _| v.fx.pending().0 || v.panel_fx.moving())) {
        if frames == 10 {
            view.update(cx, |v, cx| v.toggle_panel(true, cx));
            cx.run_until_parked();
        }
        fluid_frame(cx, w, &view);
        let (p, hang, k) = pane(cx, &view);
        assert_eq!(hang, frames < 10, "frame {frames}: the hang is the flight's end's ({k})");
        let b = bounds(cx, w, sel.clone()).expect("the label in every frame");
        let (step, moved) = ((p - last.0).abs(), (b[0] - last.1[0]).hypot(b[1] - last.1[1]));
        assert!(
            moved <= LEAD * step + std::f32::consts::SQRT_2 + 1e-3,
            "frame {frames}: the label moved {moved} px, the pane {step} ({k})"
        );
        let now = view.read_with(cx, |v, _| v.list.bounds_for_item(anchor).map(|b| f32::from(b.top()))).expect("the top row drawn");
        assert!((now - top).abs() <= 0.5, "frame {frames}: the top row moved {top} -> {now} ({k})");
        peak = peak.max(k);
        last = (p, b, k);
        frames += 1;
    }
    assert!(peak > 0.05 && peak < 0.95, "it turned half-way ({peak})");
    assert_eq!(pane(cx, &view), (p0, false, 0.), "back where it began");
    let (tops, label) = (row_tops(cx, &view), bounds(cx, w, sel.clone()));
    view.update(cx, |v, cx| {
        v.list.remeasure();
        cx.notify();
    });
    settle(cx, w, 8);
    assert_eq!(row_tops(cx, &view), tops, "the rows as a fresh layout has them");
    assert_eq!(bounds(cx, w, sel), label, "the label where a fresh layout puts it");
    assert!(settle(cx, w, 8) <= 1, "no frame at rest");
    // `HangFx` itself, on these widths: a flight turned back to where it
    // began lands measured again (the rows laid out on the way).
    let (lay, m) = view.read_with(cx, |v, cx| (v.layout, crate::theme::metrics(cx)));
    assert!(crate::turns::hangs(wide, &m) && !crate::turns::hangs(p0, &m), "{p0} inline, {wide} hanging");
    let mut fx = crate::turns::HangFx::default();
    assert!(!fx.frame(lay, p0, p0, false, &m).1, "at rest");
    let mut landed = Vec::new();
    for (pane, to) in [(p0, wide), (p0 + 40., wide), (p0 + 80., wide), (p0 + 60., p0), (p0 + 30., p0), (p0, p0)] {
        let (l, again) = fx.frame(lay, pane, to, true, &m);
        landed.push((l.hang_k, again));
    }
    assert_eq!(landed[0], (0., false), "{landed:?}");
    assert!(landed[1..5].iter().all(|&(k, again)| k > 0. && k < 1. && !again), "on the way, turned: {landed:?}");
    assert_eq!(landed[5], (0., true), "turned back, measured again as it lands: {landed:?}");
}

/// A room held at the end (the demo's card answered elsewhere while the
/// list follows, the turn running) as the rail closes across the hang
/// (Shady #33 M1): the room stays held and the list at its end, every
/// frame; the column above it holds still but for the label lines closing
/// (no row moves more than this frame's share of them, the top row's
/// least); landed, no frame at rest.
#[gpui_kit::test]
fn a_held_room_holds_through_a_flight(cx: &mut TestAppContext) {
    let (w, view) = open_one(cx, true);
    view.update(cx, |v, cx| {
        v.client = None;
        v.model.transcript.set_busy(true);
        v.list.scroll_to_end();
        cx.notify();
    });
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
    for _ in 0..120 {
        fluid_frame(cx, w, &view);
    }
    let room = view.read_with(cx, |v, _| v.fold_slack.get().map(|s| s.last())).expect("a room held at the end");
    let short = |cx: &mut TestAppContext| {
        view.read_with(cx, |v, cx| {
            let pad = crate::app::list_pad_b(&crate::theme::metrics(cx));
            f32::from(v.list.max_offset_for_scrollbar().y) + pad + f32::from(v.list.scroll_px_offset_for_scrollbar().y)
        })
    };
    assert!(short(cx) <= 1., "at its end ({room} px held)");
    let line = view.read_with(cx, |_, cx| {
        let m = crate::theme::metrics(cx);
        m.body_line.max(m.mono_line) + (2. * m.r).round()
    });
    view.update(cx, |v, cx| v.toggle_panel(true, cx));
    cx.run_until_parked();
    let mut last = (row_tops(cx, &view), 0f32, room);
    let frames = fly(cx, w, &view, |cx| {
        let (_, hang, k) = pane(cx, &view);
        assert!(hang, "to the gutter");
        let held = view.read_with(cx, |v, _| v.fold_slack.get().map(|s| s.last()));
        let gap = short(cx);
        // At its end, but for what the room's sums take from rows above
        // measured again on the way in view (#23 r5 M2's kind, main's at
        // its cut): a few px, never more than a line.
        // A row's whole-pixel share can round a pixel up as `k` moves: the
        // rows under the view's top grow that, and the held list (paused)
        // is that much short of its end until the room takes it.
        let taken = room - held.unwrap_or(0.);
        assert!(held.is_some() && gap <= 3. + taken && taken <= line, "held ({held:?} of {room}) at its end ({gap}) ({k})");
        let held = held.unwrap_or(0.);
        let tops = row_tops(cx, &view);
        // At its end the view's bottom holds: the rows above it come down
        // by the label lines closing in view, and by what the room's sums
        // take from rows above measured on the way (#23 r5 M2's kind).
        let share = tops.len() as f32 * line * (k - last.1).abs() + (last.2 - held).abs() + 2.;
        if let Some((r, y0)) = last.0.iter().min_by(|a, b| a.1.total_cmp(b.1))
            && let Some(y) = tops.get(r)
        {
            assert!((y - y0).abs() <= share, "the top row {r:?} moved {y0} -> {y} ({k}, a share {share})");
        }
        for (r, y) in &tops {
            if let Some(y0) = last.0.get(r) {
                assert!((y - y0).abs() <= share, "row {r:?} moved {y0} -> {y} ({k}, a share {share})");
            }
        }
        last = (tops, k, held);
    });
    assert!(frames > 30, "it flew ({frames})");
    assert!(pane(cx, &view).1, "landed hanging");
    let held = view.read_with(cx, |v, _| v.fold_slack.get().map(|s| s.last()));
    let gap = short(cx);
    assert!(held.is_some_and(|h| gap <= 3. + room - h), "still held ({held:?} of {room}), at its end ({gap})");
    assert!(settle(cx, w, 8) <= 1, "no frame at rest");
}

/// The pinned label (polish.md P15) on the flight that sends the labels to
/// the gutter (Shady #33 M2; the protected Q5 test checks the flight back):
/// every frame on the way its band is on the column as drawn, as tall as
/// the label line still is (`round((1 − k) · line)`), pinned at the list's
/// top, going down to the gutter's `pad_y` with the label; the label moves
/// no more than the pane's step; landed, it is pinned in the gutter.
#[gpui_kit::test]
fn the_pinned_label_goes_to_the_gutter_with_the_panel(cx: &mut TestAppContext) {
    let (w, view) = open_one(cx, true);
    // An agent message of three rows or more: its label row.
    let (l, id) = view.read_with(cx, |v, _| {
        let labels: Vec<usize> = (0..v.disp.len()).filter(|&i| v.disp[i].who != crate::turns::Who::None).collect();
        labels
            .windows(2)
            .find(|p| v.disp[p[0]].who == crate::turns::Who::Agent && p[1] - p[0] > 2)
            .map(|p| (p[0], v.disp[p[0]].id))
            .expect("a long agent message")
    });
    let at = |cx: &mut TestAppContext, off: f32| {
        view.update(cx, |v, cx| {
            v.list.scroll_to(gpui_kit::ListOffset { item_ix: l, offset_in_item: px(off) });
            cx.notify();
        });
        cx.run_until_parked();
        settle(cx, w, 4);
    };
    let viewport = |cx: &mut TestAppContext| {
        view.read_with(cx, |v, _| {
            let b = v.list.viewport_bounds();
            (f32::from(b.origin.x), f32::from(b.origin.y))
        })
    };
    at(cx, 0.);
    let off = bounds(cx, w, format!("label {id}")).expect("the label line")[1] - viewport(cx).1;
    at(cx, off + 1.);
    let pinned = format!("sticky {id}");
    let (line, pad_y) = view.read_with(cx, |_, cx| {
        let m = crate::theme::metrics(cx);
        (m.body_line, m.pad_y)
    });
    let (x, top) = viewport(cx);
    let band = bounds(cx, w, pinned.clone()).expect("pinned inline");
    let (left, column) = view.read_with(cx, |v, _| (v.layout.left, v.layout.column));
    assert!((band[0] - (x + left)).abs() < 1e-3 && (band[1] - top).abs() < 1e-3 && (band[3] - line).abs() < 1e-3, "on its band: {band:?}");
    let _ = column;
    view.update(cx, |v, cx| v.toggle_panel(true, cx));
    cx.run_until_parked();
    let mut last: (f32, Option<[f32; 4]>) = (pane(cx, &view).0, None);
    let mut between = 0;
    let frames = fly(cx, w, &view, |cx| {
        let (p, hang, k) = pane(cx, &view);
        assert!(hang, "to the gutter");
        let (x, top) = viewport(cx);
        let (left, column) = view.read_with(cx, |v, _| (v.layout.left, v.layout.column));
        let band = bounds(cx, w, pinned.clone()).expect("pinned in every frame");
        let label = bounds(cx, w, format!("sticky-label {id}"));
        if k > 0. && k < 1. {
            between += 1;
            let h = ((1. - k) * line).round();
            assert!(
                (band[0] - (x + left)).abs() < 1e-3 && (band[2] - column).abs() < 1e-3 && (band[3] - h).abs() < 1e-3,
                "on the column as drawn, {h} px tall: {band:?} {x} {left} {column} ({k})"
            );
            assert!(band[1] >= top - 1e-3, "never over the list's top: {band:?} {top} ({k})");
            let label = label.expect("the label on its way");
            if let Some(b) = last.1 {
                let (step, moved) = ((p - last.0).abs(), (label[0] - b[0]).hypot(label[1] - b[1]));
                assert!(moved <= LEAD * step + std::f32::consts::SQRT_2 + 1e-3, "the label moved {moved} px, the pane {step} ({k})");
            }
            last = (p, Some(label));
        } else {
            last = (p, None);
        }
    });
    assert!(frames > 30 && between > 30, "it flew ({frames} frames, {between} between)");
    let (x, top) = viewport(cx);
    let left = view.read_with(cx, |v, _| v.layout.left);
    let slot = bounds(cx, w, pinned).expect("pinned in the gutter");
    assert!(slot[0] + slot[2] <= x + left + 1e-3 && (slot[1] - (top + pad_y)).abs() < 1e-3, "in the gutter at pad_y: {slot:?} {x} {left}");
    assert!(settle(cx, w, 8) <= 1, "no frame at rest");
}

/// The finish gate of #33, F1: a panel's flight ends on the layout it
/// lands at. Closing the right panel at the end, three frames after it
/// looked still, the panel stepped its last pixel as it landed and the
/// column with it (1 px right); closing the rail, the tab (1 px left). The
/// last frame that moves draws the landed widths, so the frame it lands in
/// draws the same: the pane, the column, the rows and the tab all 0 px from
/// it, and every frame after. Both panels, closing and opening, at Full
/// (with frames between) and Reduced (one frame), as the clips have it:
/// 1920 px, text 17, the rail at 480 and the right panel at 520.
#[gpui_kit::test]
fn a_panel_flight_ends_on_the_layout_it_lands_at(cx: &mut TestAppContext) {
    let (w, view, _events) = open_at(cx, (1920., 1080.), 17);
    view.update(cx, |v, cx| {
        (v.panels.rail_width, v.panels.right_width) = (Some(480), Some(520));
        cx.notify();
    });
    wait(cx, w, &view, 3000);
    let tab = view.read_with(cx, |v, _| v.model.tabs.iter().next().map(|t| t.id).expect("a tab"));
    // As drawn: the panels, the pane (the list's viewport), the column's
    // left, the rows on screen, their labels and times, the tab.
    let drawn = |cx: &mut TestAppContext| -> (bool, Vec<(String, [f32; 4])>) {
        let (moving, mut at, ids) = view.read_with(cx, |v, _| {
            let vb = v.list.viewport_bounds();
            let f = |b: gpui_kit::Bounds<gpui_kit::Pixels>| {
                [f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height)]
            };
            let mut at = vec![
                ("panels".to_string(), [v.panel_fx.sides[0].shown, v.panel_fx.sides[1].shown, v.layout.left, v.layout.column]),
                ("pane".to_string(), f(vb)),
            ];
            let mut ids = Vec::new();
            for ix in 0..v.list.item_count() {
                if let Some(b) = v.list.bounds_for_item(ix).filter(|b| b.bottom() > vb.top() && b.top() < vb.bottom()) {
                    let id = v.disp[ix].id;
                    at.push((format!("row {id}"), f(b)));
                    ids.push(id);
                }
            }
            (v.panel_fx.moving(), at, ids)
        });
        for sel in ids.iter().flat_map(|id| [format!("label {id}"), format!("time {id}")]).chain([format!("tab {tab}")]) {
            if let Some(b) = bounds(cx, w, sel.clone()) {
                at.push((sel, b));
            }
        }
        (moving, at)
    };
    // What differs, if anything.
    let apart = |a: &[(String, [f32; 4])], b: &[(String, [f32; 4])]| -> Vec<String> {
        let mut out: Vec<String> =
            a.iter().zip(b).filter(|(x, y)| x != y).map(|(x, y)| format!("{} {:?} -> {} {:?}", x.0, x.1, y.0, y.1)).collect();
        if a.len() != b.len() {
            out.push(format!("{} drawn -> {}", a.len(), b.len()));
        }
        out
    };
    let mut fails = Vec::new();
    for level in [Level::Full, Level::Reduced] {
        motion_level(cx, &view, level);
        settle(cx, w, 8);
        for rail in [true, false] {
            for close in [true, false] {
                let case = format!("{level:?} {} {}", if rail { "rail" } else { "right" }, if close { "close" } else { "open" });
                view.update(cx, |v, cx| v.toggle_panel(rail, cx));
                cx.run_until_parked();
                let mut frames = vec![drawn(cx)];
                fly(cx, w, &view, |cx| frames.push(drawn(cx)));
                for _ in 0..8 {
                    fluid_frame(cx, w, &view);
                    frames.push(drawn(cx));
                }
                let landed = frames.iter().position(|f| !f.0).unwrap_or_else(|| panic!("{case}: it lands"));
                let end = &frames[landed].1;
                if level == Level::Full {
                    assert!(landed > 30, "{case}: it flew ({landed} frames)");
                    let off = apart(&frames[landed - 1].1, end);
                    if !off.is_empty() {
                        fails.push(format!("{case}: the last frame that moves is not the landed layout (it lands at {landed}): {off:?}"));
                    }
                } else {
                    assert_eq!(landed, 0, "{case}: in one frame");
                }
                for (n, f) in frames.iter().enumerate().skip(landed + 1) {
                    let off = apart(end, &f.1);
                    if !off.is_empty() {
                        fails.push(format!("{case}: frame {n}, after it landed at {landed}: {off:?}"));
                        break;
                    }
                }
            }
        }
    }
    assert!(fails.is_empty(), "{fails:#?}");
}
