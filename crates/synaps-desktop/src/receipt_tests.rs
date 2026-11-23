//! #25: an authorize receipt says what the daemon applied, not what this
//! window sent. An Allow from here can be crossed (another client's Deny, a
//! HALT, the prompt's timeout). A daemon with SynapsCLI #430 says what it
//! applied (`PromptResolved.outcome`) and the receipt takes its word; an
//! older one does not say, and the call does
//! ([`crate::authorize::applied`]): both paths are here.
//! The window is `ui_tests`' (`--offline`'s demo, a client whose daemon is
//! nowhere), its helpers as there (that file is protected).

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, TestAppContext, VisualTestContext, WindowBounds, WindowOptions, px, size,
};
use synaps_client::protocol::{PromptOutcome, SessionEvent};

use crate::app::AppView;
use crate::motion::Level;

/// `ui_tests`' window at 1440×900: the demo session attached here, its
/// prompt up, a client whose daemon is nowhere.
fn open(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<AppView>, async_channel::Receiver<synaps_client::Event>) {
    let (window, view) = cx.update(|cx| {
        gpui_kit::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        crate::fonts::register(cx);
        let (theme, raw, _, _) = crate::theme::resolve_theme(None, None, None, None, None);
        crate::theme::apply(crate::theme::Choice { theme, raw, accent: None, text: crate::theme::Panels::default().text_size }, cx);
        crate::app::bind_keys(cx);
        let bounds = Bounds { origin: Default::default(), size: size(px(1440.), px(900.)) };
        let options = WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default() };
        crate::beat::open_window(options, cx, true, |window, cx| {
            cx.new(|cx| {
                let v = AppView::new(None, Some(crate::demo::events()), window, cx);
                v.fx.pin_clock();
                v
            })
        })
        .expect("a test window")
    });
    cx.run_until_parked();
    let nowhere = std::env::temp_dir().join(format!("synaps-desktop-receipt-test-{}", std::process::id())).join("no-daemon.sock");
    let options = synaps_client::ClientOptions::new(synaps_client::Endpoint::Socket(nowhere), synaps_client::Identity::desktop());
    let (client, events) = synaps_client::Client::spawn(options).expect("a client thread");
    view.update(cx, |v, cx| {
        v.client = Some(client);
        cx.notify();
    });
    cx.run_until_parked();
    wait(cx, window, &view, 1000);
    (window, view, events)
}

fn wait(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, ms: u64) {
    let d = std::time::Duration::from_millis(ms);
    view.read_with(cx, |v, _| v.fx.set_clock(v.fx.tick() + d));
    cx.executor().advance_clock(d);
    settle(cx, w, 8);
}

fn settle(cx: &mut TestAppContext, w: AnyWindowHandle, max: usize) -> usize {
    for n in 0..max {
        let ran = cx.update_window(w, |_, window, cx| window.simulate_next_frame(cx)).expect("the window");
        cx.run_until_parked();
        if ran == 0 {
            return n;
        }
    }
    max
}

/// The motion level (`ui_tests`' `set_motion`, renamed as `test_kit`'s:
/// `one_stream_fade_policy` reads that name as a text motion policy).
fn motion_level(cx: &mut TestAppContext, view: &Entity<AppView>, level: Level) {
    cx.update(|cx| {
        cx.set_global(crate::motion::Setting { level: Some(level), lock: None });
        crate::motion::Level::apply(cx);
    });
    view.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
}

fn stream(cx: &mut TestAppContext, view: &Entity<AppView>, ev: serde_json::Value) {
    view.update(cx, |v, cx| {
        v.model.transcript.apply(&synaps_client::protocol::decode_event(&ev).expect("an event"));
        cx.notify();
    });
    cx.run_until_parked();
}

/// Allow from here at `level`, the daemon resolves the prompt, the call
/// says nothing for 120 ms (the receipt lands, its glow's window), then
/// `end` happens to the call (its tool id): every receipt drawn from the
/// answer on (row, word, lit) and the receipt's row, and the window (to
/// read what is drawn).
fn allow_then(
    cx: &mut TestAppContext,
    level: Level,
    end: impl FnOnce(&mut TestAppContext, &Entity<AppView>, &str),
) -> (Vec<(u64, &'static str, bool)>, u64, AnyWindowHandle) {
    allow_resolved_then(cx, level, None, end)
}

/// [`allow_then`], the prompt resolved with `outcome` (SynapsCLI #430;
/// `None`: a daemon before it).
fn allow_resolved_then(
    cx: &mut TestAppContext,
    level: Level,
    outcome: Option<PromptOutcome>,
    end: impl FnOnce(&mut TestAppContext, &Entity<AppView>, &str),
) -> (Vec<(u64, &'static str, bool)>, u64, AnyWindowHandle) {
    let (w, view, _events) = open(cx);
    motion_level(cx, &view, level);
    settle(cx, w, 8);
    let key = view.read_with(cx, |v, _| v.current_card().expect("the demo's card").key);
    // The card's arm ([`crate::app::Arm`], real time).
    std::thread::sleep(crate::app::ARM + std::time::Duration::from_millis(60));
    crate::authorize::RECEIPT_LIT.with(|l| l.borrow_mut().clear());
    assert!(view.update(cx, |v, _| v.answer(key, Some(crate::model::CONFIRM_YES.into()))), "sent");
    view.update(cx, |v, cx| {
        v.model.transcript.apply(&SessionEvent::PromptResolved { prompt_id: key.1, outcome });
        cx.notify();
    });
    cx.run_until_parked();
    for _ in 0..8 {
        wait(cx, w, &view, 15);
    }
    let (row, call) = view.read_with(cx, |v, _| {
        let r = v.receipts.values().flatten().last().expect("a receipt");
        assert_eq!(r.answer, crate::authorize::Answer::Allowed, "this window sent a yes");
        let items = v.model.transcript.items();
        let t = items[crate::authorize::call_ix(items, &r.call).expect("its call")].tool.clone().expect("a call");
        assert!(t.result.is_empty() && !t.status.is_some_and(|s| s.is_terminal()), "silent so far: {t:?}");
        (r.row, r.call.to_string())
    });
    end(cx, &view, &call);
    for _ in 0..4 {
        wait(cx, w, &view, 250);
    }
    let seen = crate::authorize::RECEIPT_LIT.with(|l| l.take()).into_iter().filter(|s| s.0 == row).collect();
    (seen, row, w)
}

fn result(cx: &mut TestAppContext, view: &Entity<AppView>, call: &str, text: &str) {
    stream(cx, view, serde_json::json!({"ev": "stream", "event": {"kind": "llm", "llm": "tool_result", "tool_id": call, "result": text}}));
}

fn drawn(cx: &mut TestAppContext, w: AnyWindowHandle, sel: String) -> bool {
    VisualTestContext::from_window(w, cx).debug_bounds(String::leak(sel)).is_some()
}

/// The prompt resolved, this window's Allow crossed: the daemon applied
/// another client's Deny (or the prompt timed out): the call ends
/// `Tool call confirmation denied`. The receipt says `denied`, in no frame
/// lit, at Full and Off; and while the call said nothing yet, it was not
/// lit either (on `cf28780`: `allowed`, lit from the landing on).
#[gpui_kit::test]
fn an_allow_crossed_by_a_deny_reads_denied_and_never_lights(cx: &mut TestAppContext) {
    for level in [Level::Full, Level::Off] {
        let (seen, row, w) = allow_then(cx, level, |cx, view, call| {
            result(cx, view, call, "Tool call blocked by extension: Tool call confirmation denied: guard wants to check this command");
        });
        assert!(!seen.is_empty(), "{level:?}: the receipt was drawn");
        assert!(seen.iter().all(|s| !s.2), "{level:?}: never lit: {seen:?}");
        assert_eq!(seen.last().map(|s| s.1), Some("denied"), "{level:?}: {seen:?}");
        assert!(drawn(cx, w, format!("receipt-word {row} denied")), "{level:?}: the row says denied");
        assert!(!drawn(cx, w, format!("receipt-word {row} allowed")), "{level:?}: never allowed");
        cx.update_window(w, |_, w, _| w.remove_window()).expect("the window");
        cx.run_until_parked();
    }
}

/// A HALT crossed this window's Allow: the call is cancelled with nothing
/// run (the engine's bare `Canceled by user`), or the turn ends before the
/// call reports. `cancelled`, never lit (on `cf28780`: `allowed`, lit).
#[gpui_kit::test]
fn an_allow_crossed_by_a_halt_reads_cancelled_and_never_lights(cx: &mut TestAppContext) {
    for (level, how) in [(Level::Full, "result"), (Level::Full, "turn end"), (Level::Off, "result")] {
        let (seen, row, w) = allow_then(cx, level, |cx, view, call| {
            if how == "result" {
                result(cx, view, call, "Canceled by user");
            } else {
                view.update(cx, |v, cx| {
                    v.model.transcript.apply(&SessionEvent::Idle);
                    cx.notify();
                });
                cx.run_until_parked();
            }
        });
        assert!(seen.iter().all(|s| !s.2), "{level:?} {how}: never lit: {seen:?}");
        assert_eq!(seen.last().map(|s| s.1), Some("cancelled"), "{level:?} {how}: {seen:?}");
        assert!(drawn(cx, w, format!("receipt-word {row} cancelled")), "{level:?} {how}");
        assert!(!drawn(cx, w, format!("receipt-word {row} allowed")), "{level:?} {how}: never allowed");
        cx.update_window(w, |_, w, _| w.remove_window()).expect("the window");
        cx.run_until_parked();
    }
}

/// Applied: the call's output streams (past its gate), and the receipt
/// lights then, at Full. At Off it says `allowed`, unlit (nothing lights
/// at Off). (A cancel after it ran stays `allowed`: `authorize`'s
/// `a_receipt_says_what_and_when`.)
#[gpui_kit::test]
fn an_applied_allow_lights_when_its_call_runs_and_stays_allowed(cx: &mut TestAppContext) {
    for level in [Level::Full, Level::Off] {
        let (seen, row, w) = allow_then(cx, level, |cx, view, call| {
            let delta = serde_json::json!({"ev": "stream", "event": {"kind": "llm", "llm": "tool_result_delta", "tool_id": call, "delta": "removed 3 files\n"}});
            stream(cx, view, delta);
        });
        let unlit_before = seen.iter().take_while(|s| !s.2).count();
        assert!(unlit_before > 0, "{level:?}: unlit while the call said nothing: {seen:?}");
        assert_eq!(seen.iter().any(|s| s.2), level == Level::Full, "{level:?}: lit once it ran (Full only): {seen:?}");
        assert!(seen.iter().all(|s| s.1 == "allowed"), "{level:?}: {seen:?}");
        assert!(drawn(cx, w, format!("receipt-word {row} allowed")), "{level:?}");
        cx.update_window(w, |_, w, _| w.remove_window()).expect("the window");
        cx.run_until_parked();
    }
}

// ── SynapsCLI #430: the daemon says what it applied ─────────────────────

/// The daemon applied another client's Deny over this window's Allow: the
/// receipt says `denied` from its landing, while the call has said nothing
/// (without the outcome it could say only `allowed`, unlit, until the call
/// ended), and never lights, at Full and Off.
#[gpui_kit::test]
fn a_reported_deny_reads_denied_from_the_landing(cx: &mut TestAppContext) {
    for level in [Level::Full, Level::Off] {
        let (seen, row, w) = allow_resolved_then(cx, level, Some(PromptOutcome::Denied), |_, _, _| {});
        assert!(!seen.is_empty(), "{level:?}: the receipt was drawn");
        assert!(seen.iter().all(|s| s.1 == "denied" && !s.2), "{level:?}: denied in every frame, unlit: {seen:?}");
        assert!(drawn(cx, w, format!("receipt-word {row} denied")), "{level:?}");
        cx.update_window(w, |_, w, _| w.remove_window()).expect("the window");
        cx.run_until_parked();
    }
}

/// The daemon applied this window's Allow: lit as it lands at Full (no
/// wait for the call to say something), and still `allowed` when a HALT
/// stops the call after it ran silently, though the engine's bare
/// `Canceled by user` alone would read as a cancel at the gate. At Off,
/// `allowed`, never lit.
#[gpui_kit::test]
fn a_reported_allow_lights_as_it_lands_and_outlives_a_halt(cx: &mut TestAppContext) {
    for level in [Level::Full, Level::Off] {
        let (seen, row, w) = allow_resolved_then(cx, level, Some(PromptOutcome::Allowed), |cx, view, call| {
            result(cx, view, call, "Canceled by user");
        });
        assert!(seen.iter().all(|s| s.1 == "allowed"), "{level:?}: {seen:?}");
        if level == Level::Full {
            assert!(seen.first().is_some_and(|s| s.2), "Full: lit from the landing: {seen:?}");
        } else {
            assert!(seen.iter().all(|s| !s.2), "Off: never lit: {seen:?}");
        }
        assert!(drawn(cx, w, format!("receipt-word {row} allowed")), "{level:?}");
        assert!(!drawn(cx, w, format!("receipt-word {row} cancelled")), "{level:?}");
        cx.update_window(w, |_, w, _| w.remove_window()).expect("the window");
        cx.run_until_parked();
    }
}

/// The daemon resolved the prompt as cancelled (a HALT reached it first),
/// and the gate's `None` reads as a denial in the call's result: the
/// receipt says `cancelled`, not `denied`, never lit, at Full and Off.
#[gpui_kit::test]
fn a_reported_cancel_reads_cancelled_though_the_call_reads_denied(cx: &mut TestAppContext) {
    for level in [Level::Full, Level::Off] {
        let (seen, row, w) = allow_resolved_then(cx, level, Some(PromptOutcome::Cancelled), |cx, view, call| {
            result(cx, view, call, "Tool call blocked by extension: Tool call confirmation denied: guard wants to check this command");
        });
        assert!(seen.iter().all(|s| s.1 == "cancelled" && !s.2), "{level:?}: {seen:?}");
        assert!(drawn(cx, w, format!("receipt-word {row} cancelled")), "{level:?}");
        assert!(!drawn(cx, w, format!("receipt-word {row} denied")), "{level:?}");
        cx.update_window(w, |_, w, _| w.remove_window()).expect("the window");
        cx.run_until_parked();
    }
}
