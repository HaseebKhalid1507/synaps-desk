//! A side panel's flight that widens the pane (Haseeb, 2026-11-26: closing
//! the rail kept the transcript's old, narrow column for the whole flight
//! and jumped to the new width as it landed). The column is the one it
//! lands with from the flight's first frame, laid out in the pane as it
//! lands, and the panel's edge reveals it ([`crate::turns::Reveal`]): its
//! far edge holds, and it goes on to its landed place with the pane. One
//! rewrap a flight, at its start; a flight turned round rewraps once more,
//! as it turns. Opening (the pane narrowing) is as before. Every frame of
//! the flight at 144 and at 60 Hz, at Full; the `--offline` demo.
//!
//! The helpers are `test_kit`'s, from [`super`].

use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext, VisualTestContext};
use synaps_client::protocol::{LlmEvent, SessionEvent, StreamEvent};

use super::{motion_level, open_at, settle, wait};
use crate::app::AppView;
use crate::motion::Level;

/// The demo at `w` px, the rail open at `rail` px and the right panel
/// folded, at Full: short turns after the demo's, so the column is full
/// of text that wraps.
fn opened(cx: &mut TestAppContext, w: f32, rail: u16) -> (AnyWindowHandle, Entity<AppView>) {
    let (win, view, _events) = open_at(cx, (w, 800.), crate::theme::Panels::default().text_size);
    motion_level(cx, &view, Level::Full);
    view.update(cx, |v, cx| {
        v.panels.rail_collapsed = false;
        v.panels.right_collapsed = true;
        v.panels.rail_width = Some(rail);
        for n in 0..8 {
            let t = &mut v.model.transcript;
            let user_text = Some(format!("Question {n}: where does the refresh race start, and what holds the lock while it runs?"));
            t.apply(&SessionEvent::TurnStarted { turn_baseline: 12 + 2 * n, trigger: "user".into(), user_text });
            let text = format!(
                "Answer {n}: in the relay's token cache, two lines down from the lock, where the refresh reads the expiry \
                 before it takes the lock and writes the new token after it lets go."
            );
            t.apply(&SessionEvent::Stream { event: StreamEvent::Llm(LlmEvent::Text { text }) });
            t.apply(&SessionEvent::Idle);
        }
        v.sync_list();
        v.list.scroll_to_end();
        cx.notify();
    });
    wait(cx, win, &view, 3000);
    (win, view)
}

/// One frame `hz` times a second later, drawn (`test_kit::fluid_frame` at
/// any rate).
fn frame_at(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, hz: u32) {
    let dt = std::time::Duration::from_micros(1_000_000 / u64::from(hz));
    let (t0, at) = (cx.executor().now(), view.read_with(cx, |v, _| v.fx.clock()));
    let _end = cx.executor().timer(dt);
    loop {
        cx.dispatcher.advance_clock_to_next_timer();
        let t = cx.executor().now() - t0;
        view.read_with(cx, |v, _| v.fx.set_clock(at + t));
        cx.run_until_parked();
        if t >= dt {
            break;
        }
    }
    view.update(cx, |_, cx| cx.notify());
    cx.update_window(w, |_, window, cx| window.simulate_next_frame(cx)).expect("the window");
    cx.run_until_parked();
}

/// A frame as drawn: whether a panel was in flight, the pane's left edge
/// and width, the column's width, the column's left edge in the window
/// (the list's row `ix` as drawn), the list's item width there, and the
/// composer's left and right edges. Cross-fading its rewrap
/// ([`crate::turns::Fade`]): the fade's strength, the faded-out rows' row
/// `ix` as drawn (its left edge, its width), and whether its text views
/// are their own (none the pane's).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Drawn {
    moving: bool,
    pane: (f32, f32),
    column: f32,
    x: f32,
    item: f32,
    composer: (f32, f32),
    fade: Option<Faded>,
}

/// A frame of a rewrap's cross-fade ([`Drawn::fade`]).
type Faded = (f32, Option<(f32, f32)>, bool);

fn drawn(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, ix: usize) -> Drawn {
    let composer = VisualTestContext::from_window(w, cx).debug_bounds("composer-frame 0").expect("the composer");
    view.read_with(cx, |v, _| {
        let rail = v.panel_fx.sides[0].shown;
        let b = v.list.bounds_for_item(ix).expect("the row drawn");
        Drawn {
            moving: v.panel_fx.moving(),
            pane: (crate::chrome::MARK + rail, v.width - crate::chrome::MARK - rail - v.panel_fx.sides[1].shown),
            column: v.layout.column,
            x: f32::from(b.left()),
            item: f32::from(b.size.width),
            composer: (f32::from(composer.left()), f32::from(composer.right())),
            fade: v.panes.cur.fade.as_ref().map(|f| {
                let row = f.list.bounds_for_item(ix).map(|b| (f32::from(b.left()), f32::from(b.size.width)));
                let texts = v.texts.borrow();
                let own = f.texts.borrow().iter().all(|(id, g)| texts.view(*id).is_none_or(|(v, _)| v.entity_id() != g.entity_id()));
                (f.k.get(), row, own && !f.texts.borrow().is_empty())
            }),
        }
    })
}

/// The rail toggled at `hz`: the frame before, then every frame until
/// nothing moves (at most 400); `turn`: toggled again that many frames in.
fn toggle(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, hz: u32, turn: Option<usize>) -> Vec<Drawn> {
    // The last row: at the list's end it is drawn whatever the wrap.
    let ix = view.read_with(cx, |v, _| v.disp.len() - 1);
    let mut seen = vec![drawn(cx, w, view, ix)];
    view.update(cx, |v, cx| v.toggle_panel(true, cx));
    cx.run_until_parked();
    let mut n = 0;
    while n < 400 && (n < 2 || view.read_with(cx, |v, _| v.fx.pending().0 || v.panel_fx.moving())) {
        if Some(n) == turn {
            view.update(cx, |v, cx| v.toggle_panel(true, cx));
            cx.run_until_parked();
        }
        frame_at(cx, w, view, hz);
        seen.push(drawn(cx, w, view, ix));
        n += 1;
    }
    assert!(n < 400, "it landed");
    seen
}

/// The changes of the column along `seen`: at which frames.
fn rewraps(seen: &[Drawn]) -> Vec<usize> {
    (1..seen.len()).filter(|&i| seen[i].column != seen[i - 1].column).collect()
}

/// What every frame of a flight keeps: the list's item width is the
/// column (bit for bit: the list keeps its rows' heights), and the
/// composer is on the column (the chrome and the rows agree).
fn every_frame(case: &str, seen: &[Drawn]) {
    for (i, d) in seen.iter().enumerate() {
        assert_eq!(d.item, d.column, "{case}, frame {i}: the list's item width is the column: {d:?}");
        assert!(
            (d.composer.0 - d.x).abs() < 0.01 && (d.composer.1 - (d.x + d.column)).abs() < 0.01,
            "{case}, frame {i}: the composer on the column: {d:?}"
        );
    }
}

/// The rail closing (1100 px, the rail at 400: the pane 652 to its strip's
/// 1052, the column 604 to its cap): from the flight's first frame the
/// landed column, rewrapped once; its right edge (the far one from the
/// rail) where it was, then a glide no faster than the centred column's
/// (half the pane's step) to where it lands, the last frame that moves;
/// nothing after. At 144 and 60 Hz.
#[gpui_kit::test]
fn closing_the_rail_reveals_the_landed_column_from_the_first_frame(cx: &mut TestAppContext) {
    let (w, view) = opened(cx, 1100., 400);
    for hz in [144, 60] {
        let case = format!("close at {hz} Hz");
        let seen = toggle(cx, w, &view, hz, None);
        eprintln!("REVEAL {case}: {seen:?}");
        let (before, first, last) = (seen[0], seen[1], seen[seen.len() - 1]);
        assert!(first.moving && last.column > before.column, "{case}: it flew and the column widened: {before:?} {last:?}");
        assert_eq!(first.column, last.column, "{case}: the landed column from the first frame: {first:?}, landed {last:?}");
        assert_eq!(rewraps(&seen), vec![1], "{case}: one rewrap, as it starts");
        every_frame(&case, &seen);
        // As far as a centred column's would have gone in that first step.
        let step = first.pane.1 - before.pane.1;
        assert!(
            ((first.x + first.column) - (before.x + before.column)).abs() <= step / 2. + 1.,
            "{case}: its right edge where it was: {before:?} {first:?}"
        );
        for (i, p) in seen[1..].windows(2).enumerate() {
            let step = (p[1].pane.1 - p[0].pane.1).abs();
            assert!(p[1].x <= p[0].x && p[0].x - p[1].x <= step / 2. + 1., "{case}, frame {}: a glide to the left: {p:?}", i + 2);
        }
        // Where it lands is where it rests: nothing moves after.
        let moved = seen.windows(2).rposition(|p| p[0].x != p[1].x || p[0].pane != p[1].pane).map_or(0, |i| i + 1);
        assert!(seen[moved..].iter().all(|d| *d == Drawn { moving: d.moving, ..last }), "{case}: nothing after it lands");
        assert_eq!(settle(cx, w, 8), 0, "{case}: no frame at rest");
        assert_eq!(drawn(cx, w, &view, view.read_with(cx, |v, _| v.disp.len() - 1)), Drawn { moving: false, ..last });
        // Open again for the next rate (as before: `opening_the_rail_is_as_before`).
        toggle(cx, w, &view, hz, None);
        assert_eq!(settle(cx, w, 8), 0, "{case}: opened, no frame at rest");
    }
}

/// The rail opening (the pane narrowing) is as before: the landed column
/// from the first frame, centred in the pane as drawn in every frame,
/// rewrapped once, nothing after it lands. At 144 and 60 Hz.
#[gpui_kit::test]
fn opening_the_rail_is_as_before(cx: &mut TestAppContext) {
    let (w, view) = opened(cx, 1100., 400);
    view.update(cx, |v, cx| v.toggle_panel(true, cx));
    wait(cx, w, &view, 1000);
    for hz in [144, 60] {
        let case = format!("open at {hz} Hz");
        let seen = toggle(cx, w, &view, hz, None);
        let (before, first, last) = (seen[0], seen[1], seen[seen.len() - 1]);
        assert!(first.moving && last.column < before.column, "{case}: it flew and the column narrowed");
        assert_eq!(first.column, last.column, "{case}: the landed column from the first frame");
        assert_eq!(rewraps(&seen), vec![1], "{case}: one rewrap, as it starts");
        every_frame(&case, &seen);
        for (i, d) in seen[1..].iter().enumerate() {
            let left = ((d.pane.1 - d.column) / 2.).floor();
            assert!((d.x - d.pane.0 - left).abs() <= 1., "{case}, frame {}: centred in the pane as drawn: {d:?}", i + 1);
        }
        assert_eq!(settle(cx, w, 8), 0, "{case}: no frame at rest");
        toggle(cx, w, &view, hz, None);
        assert_eq!(settle(cx, w, 8), 0, "{case}: closed again, no frame at rest");
    }
}

/// A close turned round 8 frames in (the fold key twice, #62's "takes over
/// where it is"): the landed column as it starts, the column it started
/// with again as it turns (one rewrap each), back where it began, nothing
/// after it lands. And an open turned round: the narrow column as it
/// starts, the wide one revealed from the turn. At 144 and 60 Hz.
#[gpui_kit::test]
fn a_flight_turned_round_rewraps_once_each_way(cx: &mut TestAppContext) {
    let (w, view) = opened(cx, 1100., 400);
    for hz in [144, 60] {
        let turn = if hz == 144 { 8 } else { 4 };
        for (what, narrows) in [("close", false), ("open", true)] {
            let case = format!("{what} turned round at {hz} Hz");
            let seen = toggle(cx, w, &view, hz, Some(turn));
            eprintln!("REVEAL {case}: {seen:?}");
            let (before, last) = (seen[0], seen[seen.len() - 1]);
            assert_eq!(rewraps(&seen), vec![1, turn + 1], "{case}: one rewrap as it starts, one as it turns");
            assert_eq!((last.column, last.pane), (before.column, before.pane), "{case}: back where it began");
            assert_eq!(seen[1].column < before.column, narrows, "{case}: the first leg's column");
            every_frame(&case, &seen);
            assert_eq!(settle(cx, w, 8), 0, "{case}: no frame at rest");
            // The next case starts from the other side.
            toggle(cx, w, &view, hz, None);
            assert_eq!(settle(cx, w, 8), 0);
        }
    }
}

/// At Reduced and Off the rail lands in one frame, the column with it, as
/// before: one rewrap, at once, nothing after.
#[gpui_kit::test]
fn at_reduced_and_off_the_column_lands_with_the_rail(cx: &mut TestAppContext) {
    let (w, view) = opened(cx, 1100., 400);
    for level in [Level::Reduced, Level::Off] {
        motion_level(cx, &view, level);
        settle(cx, w, 8);
        for what in ["close", "open"] {
            let case = format!("{what} at {level:?}");
            let seen = toggle(cx, w, &view, 144, None);
            let landed = seen[1];
            assert!(seen[1..].iter().all(|d| !d.moving && d.column == landed.column && d.x == landed.x), "{case}: in one frame: {seen:?}");
            assert_eq!(rewraps(&seen), vec![1], "{case}: one rewrap, at once");
            every_frame(&case, &seen);
            assert_eq!(settle(cx, w, 8), 0, "{case}: no frame at rest");
        }
    }
}

/// The rewrap cross-fades (B): in a flight's first frame the rows as the
/// frame before drew them, where they were (their column, its width), at
/// full strength and the rewrapped ones at none; on the flight's own
/// frames, for [`crate::turns::REWRAP_FADE`], the one fading out as the
/// other fades in; then one list, and nothing after the flight lands. The
/// rail closing and opening, at 144 and 60 Hz; at Reduced and Off no fade.
#[gpui_kit::test]
fn a_flights_rewrap_cross_fades(cx: &mut TestAppContext) {
    let (w, view) = opened(cx, 1100., 400);
    // The turns' arrivals over (`wait` jumps the clock; they grow on frames).
    for _ in 0..120 {
        frame_at(cx, w, &view, 60);
    }
    for hz in [144, 60] {
        for what in ["close", "open"] {
            let case = format!("{what} at {hz} Hz");
            let seen = toggle(cx, w, &view, hz, None);
            eprintln!("FADE {case}: {:?}", seen.iter().map(|d| d.fade).collect::<Vec<_>>());
            let (before, first) = (seen[0], seen[1]);
            assert_eq!(before.fade, None, "{case}: none before");
            // The toggle's own draw began it, a frame before this one: one
            // frame's worth of the fade in (`ease::INOUT`: next to none).
            let (k, rows, own) = first.fade.expect("fading from the first frame");
            assert!(own, "{case}: the faded-out rows' text views their own");
            assert!(k < 0.05 && rows == Some((before.x, before.column)), "{case}: the first frame is the frame before's rows: {first:?}");
            let n = seen[1..].iter().take_while(|d| d.fade.is_some()).count();
            let dt = 1000. / hz as f32;
            assert!((n as f32 * dt - 100.).abs() <= dt + 0.5, "{case}: {n} frames of fade, 100 ms at {hz} Hz: {seen:?}");
            let ks: Vec<f32> = seen[1..=n].iter().map(|d| d.fade.expect("fading").0).collect();
            assert!(ks.windows(2).all(|k| k[1] > k[0] && k[1] < 1.), "{case}: fading in: {ks:?}");
            for d in &seen[1..=n] {
                assert_eq!(d.fade.and_then(|f| f.1), Some((before.x, before.column)), "{case}: the rows where they were: {d:?}");
                assert!(d.fade.is_some_and(|f| f.2), "{case}: their own text views: {d:?}");
            }
            assert!(seen[n + 1..].iter().all(|d| d.fade.is_none()), "{case}: one list after the fade");
            assert!(seen[n + 1].moving, "{case}: the fade ends in the flight");
            assert_eq!(settle(cx, w, 8), 0, "{case}: no frame at rest");
        }
    }
    for level in [Level::Reduced, Level::Off] {
        motion_level(cx, &view, level);
        settle(cx, w, 8);
        for what in ["close", "open"] {
            let seen = toggle(cx, w, &view, 144, None);
            assert!(seen.iter().all(|d| d.fade.is_none()), "{what} at {level:?}: no fade: {seen:?}");
            assert_eq!(settle(cx, w, 8), 0);
        }
    }
}
