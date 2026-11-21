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
                view.update(cx, |v, cx| v.toggle_panel(rail, cx));
                cx.run_until_parked();
                let (mut between, mut worst) = (0, 0f32);
                let frames = fly(cx, w, &view, |cx| {
                    let (p, hang, k) = pane(cx, &view);
                    assert_eq!(hang, close, "{case}: the hang is the flight's end's from its start");
                    if k > 0. && k < 1. {
                        between += 1;
                    }
                    let (b, row) = at(cx);
                    let (lb, lrow) = last.1;
                    let step = (p - last.0).abs();
                    // On its row: a pixel's rounding either way.
                    let glide = (b[0] - lb[0]).hypot((b[1] - row) - (lb[1] - lrow));
                    worst = worst.max(glide - step);
                    assert!(
                        glide <= step + std::f32::consts::SQRT_2 + 1e-3,
                        "{case}: the label moved {glide} px on its row in a frame the pane moved {step} ({k})"
                    );
                    // The rows close up (open) by the label lines' room
                    // this frame (on whole pixels, rounded once), nothing more: no row jumps.
                    let tops = row_tops(cx, &view);
                    let room = tops.len() as f32 * line * (k - last.3).abs() + 2.;
                    for (r, y) in &tops {
                        if let Some(y0) = last.2.get(r) {
                            if (y - y0).abs() > room {
                                let mut a: Vec<_> = last.2.iter().collect();
                                a.sort_by(|x, y| x.1.total_cmp(y.1));
                                let mut b: Vec<_> = tops.iter().collect();
                                b.sort_by(|x, y| x.1.total_cmp(y.1));
                                eprintln!("DBG {a:?}\nDBG {b:?}");
                            }
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
                            moved <= step + std::f32::consts::SQRT_2 + 1e-3,
                            "{case}: the label moved {moved} px in a frame the pane moved {step} ({k})"
                        );
                        let now = view.read_with(cx, |v, _| v.list.bounds_for_item(anchor).map(|b| f32::from(b.top())));
                        let (a, b) = (top.expect("the top row"), now.expect("the top row drawn"));
                        assert!((a - b).abs() <= 0.5, "{case}: the top row moved {a} -> {b} ({k})");
                    }
                    last = (p, (b, row), tops, k);
                });
                assert!(frames > 30 && between > 30, "{case}: it flew ({frames} frames, {between} between)");
                eprintln!("{case}: {frames} frames, {between} between, worst label step past the pane's {worst}");
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
