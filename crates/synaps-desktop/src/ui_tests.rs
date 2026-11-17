//! UI tests: GPUI's own dispatch in a headless test window (`gpui-kit`'s
//! test support), for what the logic tests can't see: the order in which
//! GPUI runs a focused input's key bindings, the key listeners and the
//! events they emit. The window is `--offline`'s (the synthetic transport):
//! `relay-refresh` with `scout` running in the tray. Then it gets a client
//! whose daemon is nowhere (a socket path that does not exist), so the
//! composer takes Enter: what it would send is [`AppView::submits`].

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, Focusable as _, KeyDownEvent, KeyUpEvent, Keystroke, PlatformInput, TestAppContext,
    VisualTestContext, WindowBounds, WindowOptions, px, size,
};
use synaps_client::protocol::SessionEvent;

use crate::app::AppView;
use crate::wtabs::Act;

const DRAFT: &str = "half-typed, not for sending";

/// The window, and the events of its client (kept: its thread lives on).
fn open(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<AppView>, async_channel::Receiver<synaps_client::Event>) {
    open_at(cx, (1440., 900.), crate::theme::Panels::default().text_size)
}

/// [`open`], `size` px at text step `text`.
fn open_at(
    cx: &mut TestAppContext,
    (w, h): (f32, f32),
    text: u8,
) -> (AnyWindowHandle, Entity<AppView>, async_channel::Receiver<synaps_client::Event>) {
    let (window, view) = cx.update(|cx| {
        gpui_kit::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        crate::fonts::register(cx);
        let (theme, raw, _, _) = crate::theme::resolve_theme(None, None, None, None, None);
        crate::theme::apply(crate::theme::Choice { theme, raw, accent: None, text }, cx);
        crate::app::bind_keys(cx);
        let bounds = Bounds { origin: Default::default(), size: size(px(w), px(h)) };
        let options = WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default() };
        gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| AppView::new(None, Some(crate::demo::events()), window, cx)))
            .expect("a test window")
    });
    cx.run_until_parked();
    let nowhere = std::env::temp_dir().join(format!("synaps-desktop-ui-test-{}", std::process::id())).join("no-daemon.sock");
    let options = synaps_client::ClientOptions::new(synaps_client::Endpoint::Socket(nowhere), synaps_client::Identity::desktop());
    let (client, events) = synaps_client::Client::spawn(options).expect("a client thread");
    view.update(cx, |v, cx| {
        v.client = Some(client);
        assert!(v.drives(), "the demo session is attached here, its input shared");
        cx.notify();
    });
    cx.run_until_parked();
    (window, view, events)
}

fn keystroke(cx: &TestAppContext, key: &str) -> Keystroke {
    Keystroke::parse_with_os(key, cx.update(|cx| cx.platform_info().os)).expect("a keystroke")
}

/// One platform event in an update of its own, as a platform sends it:
/// what it emits is delivered, and a frame drawn, before the next.
fn event(cx: &mut TestAppContext, w: AnyWindowHandle, ev: PlatformInput) {
    cx.update_window(w, |_, window, cx| {
        window.dispatch_event(ev, cx);
    })
    .expect("the window");
    cx.run_until_parked();
}

fn down(cx: &mut TestAppContext, w: AnyWindowHandle, key: &str, held: bool) {
    let keystroke = keystroke(cx, key);
    event(cx, w, PlatformInput::KeyDown(KeyDownEvent { keystroke, is_held: held, prefer_character_input: false }));
}

fn up(cx: &mut TestAppContext, w: AnyWindowHandle, key: &str) {
    let keystroke = keystroke(cx, key);
    event(cx, w, PlatformInput::KeyUp(KeyUpEvent { keystroke }));
}

/// The composer focused, holding [`DRAFT`]; the worker menu opened from
/// the keyboard (Shift+F10) over scout's tray row, its selection on `act`.
fn menu_on(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, act: Act) {
    cx.update_window(w, |_, window, cx| {
        view.update(cx, |v, cx| {
            assert!(!v.tray.rows.is_empty(), "the demo session's tray lists scout");
            v.composer.update(cx, |s, cx| {
                s.set_value(DRAFT, window, cx);
                s.focus(window, cx);
            });
        })
    })
    .expect("the window");
    cx.run_until_parked();
    down(cx, w, "shift-f10", false);
    up(cx, w, "shift-f10");
    for _ in 0..8 {
        let at = view.read_with(cx, |v, _| v.menu.as_ref().map(|(m, _)| m.sel.map(|i| m.items[i].act)));
        match at {
            None => panic!("Shift+F10 opened no menu"),
            Some(Some(a)) if a == act => return,
            Some(_) => {
                down(cx, w, "down", false);
                up(cx, w, "down");
            }
        }
    }
    panic!("no {act:?} item that can act in the menu");
}

fn composer_text(cx: &mut TestAppContext, view: &Entity<AppView>) -> String {
    view.read_with(cx, |v, cx| v.composer.read(cx).value().to_string())
}

fn composer_focused(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) -> bool {
    cx.update_window(w, |_, window, cx| view.read(cx).composer.read(cx).focus_handle(cx).is_focused(window)).expect("the window")
}

/// R2-M1: Enter picks `act`, which shows scout in place, the focus the
/// composer's (given back, or Steer…'s own); Enter held on never steers
/// scout with the draft. A new press then does (the path is live).
fn a_held_enter_after_picking(cx: &mut TestAppContext, act: Act) {
    let (w, view, _events) = open(cx);
    menu_on(cx, w, &view, act);
    down(cx, w, "enter", false);
    assert!(view.read_with(cx, |v, _| v.menu.is_none() && v.model.watch.focus() == Some("sa_1")), "{act:?} shows scout in place");
    assert!(composer_focused(cx, w, &view), "the focus is the composer's");
    for _ in 0..4 {
        down(cx, w, "enter", true);
    }
    assert_eq!(view.read_with(cx, |v, _| v.submits.clone()), Vec::<(String, bool)>::new(), "nothing went out");
    assert_eq!(composer_text(cx, &view), DRAFT, "the draft is as it was");
    up(cx, w, "enter");
    // A new press is the user's: it steers.
    std::thread::sleep(std::time::Duration::from_millis(40));
    down(cx, w, "enter", false);
    assert_eq!(view.read_with(cx, |v, _| v.submits.clone()), vec![(DRAFT.to_string(), true)]);
}

#[gpui_kit::test]
fn a_held_enter_after_a_keyboard_open_never_steers(cx: &mut TestAppContext) {
    a_held_enter_after_picking(cx, Act::Open);
}

#[gpui_kit::test]
fn a_held_enter_after_a_keyboard_steer_never_steers(cx: &mut TestAppContext) {
    a_held_enter_after_picking(cx, Act::Steer);
}

/// R2-M1: Ctrl/Cmd+Enter picks Copy handle, the menu closes and the
/// focus goes back to the composer; held on, it never sends the draft to
/// the session (the input's Ctrl+Enter is a submit too).
#[gpui_kit::test]
fn a_held_ctrl_enter_after_a_keyboard_pick_never_sends(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    menu_on(cx, w, &view, Act::CopyHandle);
    down(cx, w, "secondary-enter", false);
    assert!(view.read_with(cx, |v, _| v.menu.is_none() && v.model.watch.focus().is_none()), "the menu is gone; the session's thread");
    assert_eq!(cx.read_from_clipboard().and_then(|c| c.text()).as_deref(), Some("sa_1"), "the handle was copied");
    assert!(composer_focused(cx, w, &view), "the focus is the composer's again");
    for _ in 0..4 {
        down(cx, w, "secondary-enter", true);
    }
    // A plain Enter's repeats once Ctrl is let go: the same held key.
    down(cx, w, "enter", true);
    assert_eq!(view.read_with(cx, |v, _| v.submits.clone()), Vec::<(String, bool)>::new(), "nothing went out");
    assert_eq!(composer_text(cx, &view), DRAFT);
    up(cx, w, "enter");
    std::thread::sleep(std::time::Duration::from_millis(40));
    down(cx, w, "enter", false);
    assert_eq!(view.read_with(cx, |v, _| v.submits.clone()), vec![(DRAFT.to_string(), false)]);
}

/// Every list swap a worker brings goes through `list_out`: its view shown
/// in place, opened in a tab, its tab closed. The text views kept for one
/// list's rows never serve the next one's (item ids restart at 1 in each),
/// and a drag on the transcript's bar ends with the list it began on.
#[gpui_kit::test]
fn a_worker_swap_carries_no_text_view_or_drag_over(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    let views = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.text_views());
    let (kept, stale) = views(cx);
    assert!(kept > 0 && stale == 0, "the session's rows drawn, their views kept: {kept}, {stale}");
    // A drag on the session's bar under way, then scout shown in place.
    let session = view.update(cx, |v, _| {
        v.list.scrollbar_drag_started();
        v.list.clone()
    });
    menu_on(cx, w, &view, Act::Open);
    down(cx, w, "enter", false);
    up(cx, w, "enter");
    assert!(view.read_with(cx, |v, _| v.model.watch.focus() == Some("sa_1")), "scout in place");
    assert!(!session.is_scrollbar_dragging(), "the drag ended with the session's list");
    assert_eq!(views(cx).1, 0, "scout's list: none of the session's views");
    // Back on the session's thread, then scout in a tab of its own.
    let scout = view.update(cx, |v, cx| {
        v.list.scrollbar_drag_started();
        let l = v.list.clone();
        v.model.watch.focus_main();
        cx.notify();
        l
    });
    cx.run_until_parked();
    assert!(!scout.is_scrollbar_dragging(), "the drag ended with scout's list");
    assert_eq!(views(cx).1, 0, "the session's list again: none of scout's views");
    menu_on(cx, w, &view, Act::OpenTab);
    down(cx, w, "enter", false);
    up(cx, w, "enter");
    assert!(view.read_with(cx, |v, _| v.model.tabs.current_worker().is_some()), "scout's tab on screen");
    assert_eq!(views(cx).1, 0, "scout's tab: none of the session's views");
    // Its tab closed: the session's list, and none of the tab's views.
    let tab = view.update(cx, |v, _| {
        v.list.scrollbar_drag_started();
        v.list.clone()
    });
    down(cx, w, "ctrl-w", false);
    up(cx, w, "ctrl-w");
    assert!(view.read_with(cx, |v, _| v.model.tabs.current_worker().is_none()), "the tab closed");
    assert!(!tab.is_scrollbar_dragging(), "the drag ended with the tab's list");
    let (kept, stale) = views(cx);
    assert!(kept > 0 && stale == 0, "the session's rows drawn again, nothing of the tab's: {kept}, {stale}");
}

/// The daemon's session poll (every 3 s, an event each time even when
/// nothing changed): the batch it comes in, as the pump applies it.
fn poll(cx: &mut TestAppContext, view: &Entity<AppView>) {
    view.update(cx, |v, cx| {
        let same = v.model.sessions.clone();
        v.apply_batch(vec![synaps_client::Event::Sessions(same)], cx);
    });
    cx.run_until_parked();
}

/// The session's one prompt answered elsewhere: no card up.
fn resolve_prompt(cx: &mut TestAppContext, view: &Entity<AppView>) {
    view.update(cx, |v, cx| {
        let ids: Vec<u64> = v.model.transcript.pending_prompts().iter().map(|p| p.id).collect();
        for prompt_id in ids {
            v.model.transcript.apply(&SessionEvent::PromptResolved { prompt_id });
        }
        cx.notify();
    });
    cx.run_until_parked();
}

fn in_place(cx: &mut TestAppContext, view: &Entity<AppView>) -> Option<String> {
    view.read_with(cx, |v, _| v.model.watch.focus().filter(|_| !v.model.watch.in_tab()).map(str::to_string))
}

/// A worker shown in place stays on screen across the session polls: the
/// session tab's hand-off (`Tab::watching`, written when it is left) is no
/// order to go back to the thread while it stays on screen.
#[gpui_kit::test]
fn a_worker_in_place_outlives_the_session_poll(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    menu_on(cx, w, &view, Act::Open);
    down(cx, w, "enter", false);
    up(cx, w, "enter");
    assert_eq!(in_place(cx, &view).as_deref(), Some("sa_1"), "scout in place");
    for _ in 0..3 {
        poll(cx, &view);
        assert_eq!(in_place(cx, &view).as_deref(), Some("sa_1"), "a poll that changed nothing left scout on screen");
    }
    // The session opened again (its rail row): its thread, as before.
    view.update(cx, |v, cx| {
        let session = v.model.tabs.current().and_then(|t| t.session.clone()).expect("its session");
        v.open_tab(crate::app::Open::Attach { session_id: session, fallback: None, open: false }, crate::tabs::Place::Current);
        cx.notify();
    });
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |v, _| v.model.watch.focus().map(str::to_string)), None, "an open of the session: its thread");
}

/// A session tab shows the worker it showed in place when it was left,
/// once: Esc'd back to the thread, the next poll does not bring it back.
#[gpui_kit::test]
fn an_escaped_worker_stays_gone_across_the_poll(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    menu_on(cx, w, &view, Act::Open);
    down(cx, w, "enter", false);
    up(cx, w, "enter");
    assert_eq!(in_place(cx, &view).as_deref(), Some("sa_1"), "scout in place");
    // The session tab left for another worker's tab, then shown again.
    let tab = view.update(cx, |v, cx| {
        let tab = v.model.tabs.current().map(|t| t.id).expect("the session's tab");
        let session = v.model.tabs.current().and_then(|t| t.session.clone()).expect("its session");
        assert!(v.model.tabs.open_worker_as(&session, "sa_9", "ghost", None, &session));
        v.reconcile(None);
        cx.notify();
        tab
    });
    cx.run_until_parked();
    let watching =
        |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.model.tabs.iter().find(|t| t.id == tab).and_then(|t| t.watching.clone()));
    assert_eq!(watching(cx), Some(("sa_1".into(), "scout".into())), "left showing scout");
    down(cx, w, "ctrl-w", false);
    up(cx, w, "ctrl-w");
    assert_eq!(view.read_with(cx, |v, _| v.model.tabs.current().map(|t| t.id)), Some(tab), "the session's tab again");
    assert_eq!(in_place(cx, &view).as_deref(), Some("sa_1"), "scout shown again, in place");
    assert_eq!(watching(cx), None, "the hand-off taken");
    poll(cx, &view);
    assert_eq!(in_place(cx, &view).as_deref(), Some("sa_1"), "and kept across a poll");
    // Esc: the thread, and it stays the thread.
    down(cx, w, "escape", false);
    up(cx, w, "escape");
    assert_eq!(view.read_with(cx, |v, _| v.model.watch.focus().map(str::to_string)), None, "Esc: the session's thread");
    for _ in 0..3 {
        poll(cx, &view);
        assert_eq!(view.read_with(cx, |v, _| v.model.watch.focus().map(str::to_string)), None, "scout did not come back");
    }
}

/// The session's tab left showing scout in place, for a tab of another
/// session (the synthetic transport answers its attach): `session`, and
/// the tab left.
fn leave_for_another_session(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) -> (String, u64) {
    menu_on(cx, w, view, Act::Open);
    down(cx, w, "enter", false);
    up(cx, w, "enter");
    assert_eq!(in_place(cx, view).as_deref(), Some("sa_1"), "scout in place");
    let left = view.update(cx, |v, cx| {
        v.client = None;
        let tab = v.model.tabs.current().map(|t| t.id).expect("the session's tab");
        let session = v.model.tabs.current().and_then(|t| t.session.clone()).expect("its session");
        let other = crate::app::Open::Attach { session_id: crate::demo::IDLE_CPU.into(), fallback: None, open: false };
        v.open_tab(other, crate::tabs::Place::New);
        cx.notify();
        (session, tab)
    });
    cx.run_until_parked();
    assert_eq!(attached(cx, view).as_deref(), Some(crate::demo::IDLE_CPU), "the other session attached");
    left
}

fn attached(cx: &mut TestAppContext, view: &Entity<AppView>) -> Option<String> {
    view.read_with(cx, |v, _| v.model.active.as_ref().map(|a| a.session_id.clone()))
}

/// M1: a session tab whose cache missed (8 sessions or 64 MiB; a larger
/// one always misses) shows the worker it showed in place when it was
/// left, and keeps it once its attach lands: its transcript is new (no
/// session yet), its view the tab's hand-off, for the session asked for.
#[gpui_kit::test]
fn a_worker_in_place_survives_a_tab_cache_miss(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    let (session, tab) = leave_for_another_session(cx, w, &view);
    view.update(cx, |v, cx| {
        assert!(v.tab_cache.take(&session).is_some(), "the session's tab was cached: now it is not");
        let ix = v.model.tabs.iter().position(|t| t.id == tab).expect("its tab");
        v.focus_tab(ix);
        cx.notify();
    });
    cx.run_until_parked();
    assert_eq!(attached(cx, &view), Some(session), "its attach landed");
    assert_eq!(in_place(cx, &view).as_deref(), Some("sa_1"), "scout in place, after the attach too");
}

/// The same with the cache kept: the tab comes back as it was left.
#[gpui_kit::test]
fn a_worker_in_place_comes_back_with_its_cached_tab(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    let (session, tab) = leave_for_another_session(cx, w, &view);
    view.update(cx, |v, cx| {
        let ix = v.model.tabs.iter().position(|t| t.id == tab).expect("its tab");
        v.focus_tab(ix);
        cx.notify();
    });
    cx.run_until_parked();
    assert_eq!(attached(cx, &view), Some(session), "its attach landed");
    assert_eq!(in_place(cx, &view).as_deref(), Some("sa_1"), "scout in place");
}

/// L1: GPUI lays every text box out a whole pixel wide (its advances,
/// rounded up), and a folded run's tally counts it that way. At every
/// text step, over the column widths up to the cap (3 px apart), the
/// last tool a tally names and its `+N` end inside it: nothing is clipped.
/// The test window's text system advances every character 0.6 em, as
/// Hybrid Mono advances every glyph it maps. Bounds are recorded in debug
/// builds only (`debug_selector`).
#[cfg(debug_assertions)]
#[gpui_kit::test]
fn a_tally_never_clips_what_it_names(cx: &mut TestAppContext) {
    use gpui_kit::VisualTestContext;
    let (w, view, _events) = open(cx);
    // The demo's session whose turns end in folded runs (2, 4 and 17
    // tools); both side panels folded: the pane is the window less them.
    view.update(cx, |v, cx| {
        v.client = None;
        v.panels.rail_collapsed = true;
        v.panels.right_collapsed = true;
        let open = crate::app::Open::Attach { session_id: crate::demo::IDLE_CPU.into(), fallback: None, open: false };
        v.open_tab(open, crate::tabs::Place::Current);
        cx.notify();
    });
    cx.run_until_parked();
    let leak = |s: String| -> &'static str { Box::leak(s.into_boxed_str()) };
    let runs: Vec<(usize, [&'static str; 3])> = view
        .read_with(cx, |v, _| {
            v.summaries
                .iter()
                .map(|(id, s)| (s.tools, [format!("tally {id}"), format!("tally {id} last"), format!("tally {id} more")]))
                .collect::<Vec<_>>()
        })
        .into_iter()
        .map(|(n, [a, b, c])| (n, [leak(a), leak(b), leak(c)]))
        .collect();
    assert!(runs.iter().any(|r| r.0 == 17), "the 17-tool run is summarised: {:?}", runs.iter().map(|r| r.0).collect::<Vec<_>>());
    let mut vcx = VisualTestContext::from_window(w, cx);
    let (mut drawn, mut cut, mut whole) = (0, 0, 0);
    for step in crate::theme::TEXT_STEPS {
        view.update(&mut vcx, |v, cx| {
            v.show_text_size(step, cx);
            cx.notify();
        });
        let cap = crate::theme::Metrics::at(step).column.floor();
        for width in (420..2000).step_by(3) {
            vcx.simulate_resize(size(px(width as f32), px(900.)));
            vcx.run_until_parked();
            let (seen, column) = view.read_with(&vcx, |v, _| (v.width, v.layout.column));
            assert_eq!(seen, width as f32, "drawn at {width} px");
            for (tools, [tally, last, more]) in &runs {
                let Some(t) = vcx.debug_bounds(tally) else { continue };
                drawn += 1;
                for (part, b) in [("last name", vcx.debug_bounds(last)), ("+N", vcx.debug_bounds(more))] {
                    if let Some(b) = b {
                        assert!(
                            b.right() <= t.right(),
                            "step {step}, column {column}: the {tools}-tool tally's {part} ends at {:?}, past its edge {:?}",
                            b.right(),
                            t.right()
                        );
                    }
                }
                if vcx.debug_bounds(more).is_some() {
                    cut += usize::from(*tools == 17);
                } else if vcx.debug_bounds(last).is_some() {
                    whole += 1;
                }
            }
            if column >= cap {
                break;
            }
        }
    }
    eprintln!("tallies drawn {drawn}: the 17-tool one cut {cut} times, a tally whole {whole} times");
    assert!(cut > 0 && whole > 0, "the sweep cuts the 17-tool tally, and shows tallies whole: {cut}, {whole}");
}

/// A new press of `key` (down and up, then past a repeat's 30 ms).
fn tap(cx: &mut TestAppContext, w: AnyWindowHandle, key: &str) {
    down(cx, w, key, false);
    up(cx, w, key);
    std::thread::sleep(std::time::Duration::from_millis(40));
}

fn focus_composer(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    cx.update_window(w, |_, window, cx| view.update(cx, |v, cx| v.composer.update(cx, |s, cx| s.focus(window, cx)))).expect("the window");
    cx.run_until_parked();
}

/// design.md §8.14: the session's state and HALT stay reachable at every
/// window size and text step: the foot's glyph and HALT (the demo's turn
/// runs, and this window drives it) are drawn inside the window at
/// 1920×1080, 1280×720, 860×600 and the smallest window, 640×420
/// (the narrowest; cramped from step 16: the inline row). The word drops only in a tight
/// pane or cramped (no foot row).
#[gpui_kit::test]
fn the_foot_keeps_its_glyph_and_halt_at_every_size(cx: &mut TestAppContext) {
    for (w, h) in [(1920., 1080.), (1280., 720.), (860., 600.), (640., 420.)] {
        for step in crate::theme::TEXT_STEPS {
            let (win, view, _events) = open_at(cx, (w, h), step);
            let lay = view.read_with(cx, |v, _| v.layout);
            let mut vcx = VisualTestContext::from_window(win, cx);
            let inside = |b: Option<gpui_kit::Bounds<gpui_kit::Pixels>>, what: &str| {
                let b = b.unwrap_or_else(|| panic!("{w}x{h} step {step}: no {what}"));
                assert!(
                    f32::from(b.origin.x) >= 0. && f32::from(b.bottom()) <= h && f32::from(b.right()) <= w && f32::from(b.size.width) > 0.,
                    "{w}x{h} step {step}: {what} at {b:?}"
                );
            };
            inside(vcx.debug_bounds("foot-glyph"), "foot glyph");
            inside(vcx.debug_bounds("halt"), "HALT");
            let word = vcx.debug_bounds("foot-word").is_some();
            assert_eq!(word, !lay.tight && !lay.cramped, "{w}x{h} step {step}: the word ({lay:?})");
            if (w, h) == (1280., 720.) && step == 16 {
                assert!(!lay.short, "1280×720 is full now");
            }
            if (w, h) == (640., 420.) {
                // Cramped below a 430-px window at 16 px (design.md §8.14).
                assert!(lay.short && (step < 16 || lay.cramped), "the smallest window at step {step}: {lay:?}");
            }
        }
    }
}

/// HALT goes away when this window does not drive the turn (no client).
#[gpui_kit::test]
fn halt_is_not_drawn_without_a_turn_this_window_drives(cx: &mut TestAppContext) {
    let (win, view, _events) = open(cx);
    view.update(cx, |v, cx| {
        v.client = None;
        cx.notify();
    });
    cx.run_until_parked();
    let mut vcx = VisualTestContext::from_window(win, cx);
    assert!(vcx.debug_bounds("halt").is_none());
    assert!(vcx.debug_bounds("foot-glyph").is_some(), "the glyph stays");
}

/// Esc, Esc in the composer halts the turn, once; a held Esc never does;
/// with the card up the first Esc goes to the card (no HALT); from a
/// worker's view Esc goes back and the Esc right after is not HALT's.
#[gpui_kit::test]
fn esc_esc_halts_once(cx: &mut TestAppContext) {
    // The card up: Esc leaves the composer for the card's keys.
    let (w, view, _events) = open(cx);
    focus_composer(cx, w, &view);
    tap(cx, w, "escape");
    tap(cx, w, "escape");
    assert_eq!(view.read_with(cx, |v, _| v.halts), 0, "the card's Esc, not HALT's");
    // No card: the first arms, the second halts.
    resolve_prompt(cx, &view);
    focus_composer(cx, w, &view);
    tap(cx, w, "escape");
    assert!(view.read_with(cx, |v, _| v.halts == 0 && v.halt_armed.is_some()), "armed");
    tap(cx, w, "escape");
    assert_eq!(view.read_with(cx, |v, _| (v.halts, v.halt_armed)), (1, None), "halted once");
    // Held: its repeats are not presses.
    down(cx, w, "escape", false);
    for _ in 0..4 {
        down(cx, w, "escape", true);
    }
    up(cx, w, "escape");
    assert_eq!(view.read_with(cx, |v, _| v.halts), 1, "a held Esc halts nothing");
    std::thread::sleep(crate::app::ARM);
    // From a worker's view: back to the session; the Esc right after
    // (inside its guard) arms nothing.
    view.update(cx, |v, _| v.halt_armed = None);
    menu_on(cx, w, &view, Act::Open);
    down(cx, w, "enter", false);
    up(cx, w, "enter");
    assert!(view.read_with(cx, |v, _| v.model.watch.focus() == Some("sa_1")));
    focus_composer(cx, w, &view);
    tap(cx, w, "escape");
    assert!(view.read_with(cx, |v, _| v.model.watch.focus().is_none()), "Esc went back");
    tap(cx, w, "escape");
    assert!(view.read_with(cx, |v, _| v.halts == 1 && v.halt_armed.is_none()), "the worker view's Esc and its guard arm nothing");
}

/// F2 opens (and closes) the naming row of the session on screen, from
/// the window or from the composer.
#[gpui_kit::test]
fn f2_opens_the_naming_row(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    let naming = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.model.named.naming);
    assert!(!naming(cx));
    tap(cx, w, "f2");
    assert!(naming(cx), "F2 opens it");
    tap(cx, w, "f2");
    assert!(!naming(cx), "and closes it");
    focus_composer(cx, w, &view);
    tap(cx, w, "f2");
    assert!(naming(cx), "from the composer too");
}

/// The NEEDS YOU stub (design.md §8.17) is drawn only over a worker's view
/// while its session waits on a prompt; the card is not; a click on it
/// goes back to the session with the card up.
#[gpui_kit::test]
fn the_stub_is_only_over_a_worker_view_with_a_waiting_prompt(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    let stub = |cx: &mut TestAppContext| VisualTestContext::from_window(w, cx).debug_bounds("needs-stub");
    assert!(stub(cx).is_none(), "the session's thread: the card itself");
    menu_on(cx, w, &view, Act::Open);
    down(cx, w, "enter", false);
    up(cx, w, "enter");
    assert!(view.read_with(cx, |v, _| v.model.watch.focus() == Some("sa_1")));
    let at = stub(cx).expect("the stub over scout's view");
    assert!(VisualTestContext::from_window(w, cx).debug_bounds("foot-main").is_some(), "MAIN › in the foot");
    // A click on it: the session's thread, its card up.
    let centre = at.center();
    event(
        cx,
        w,
        PlatformInput::MouseDown(gpui_kit::MouseDownEvent {
            button: gpui_kit::MouseButton::Left,
            position: centre,
            modifiers: Default::default(),
            click_count: 1,
            first_mouse: false,
        }),
    );
    event(
        cx,
        w,
        PlatformInput::MouseUp(gpui_kit::MouseUpEvent {
            button: gpui_kit::MouseButton::Left,
            position: centre,
            modifiers: Default::default(),
            click_count: 1,
        }),
    );
    assert!(view.read_with(cx, |v, _| v.model.watch.focus().is_none()), "back on the session's thread");
    assert!(view.read_with(cx, |v, _| v.current_card().is_some()), "its card up");
    assert!(stub(cx).is_none());
    // No prompt waiting: no stub over the worker either.
    resolve_prompt(cx, &view);
    menu_on(cx, w, &view, Act::Open);
    down(cx, w, "enter", false);
    up(cx, w, "enter");
    assert!(view.read_with(cx, |v, _| v.model.watch.focus() == Some("sa_1")));
    assert!(stub(cx).is_none(), "nothing waits");
}

/// Scout (sa_1) shown in place over the session's thread.
fn scout_in_place(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    menu_on(cx, w, view, Act::Open);
    down(cx, w, "enter", false);
    up(cx, w, "enter");
    assert_eq!(in_place(cx, view).as_deref(), Some("sa_1"), "scout in place");
}

/// What the foot says of the worker on screen: its state and facts.
fn foot_of(cx: &mut TestAppContext, view: &Entity<AppView>) -> (crate::subagents::Liveness, String) {
    view.read_with(cx, |v, _| {
        let w = v.model.watch.focused().expect("a worker on screen");
        let (live, facts) = v.watched_state(w);
        (live, facts.to_string())
    })
}

/// Its view polled `running` from the daemon (the offline demo's view
/// has no daemon to poll): the page, and what the pump does with it.
fn view_running(cx: &mut TestAppContext, view: &Entity<AppView>) {
    view.update(cx, |v, _| {
        let w = v.model.watch.focused_mut().expect("a worker on screen");
        w.gone = None;
        w.status = "running".into();
        let handle = w.handle_id.clone();
        v.model.heard_view(&handle);
    });
}

/// Shady M1: a cold-started daemon numbers its workers from 1 again, and
/// sa_1 is another worker now. The foot over scout's view says GONE (as
/// scout's tab would), and none of the new sa_1's facts.
#[gpui_kit::test]
fn the_foot_never_says_another_worker_under_its_handle(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    scout_in_place(cx, w, &view);
    view_running(cx, &view);
    let tray = view.read_with(cx, |v, _| v.tray.rows.iter().find(|r| r.id == 1).map(|r| r.facts.to_string())).expect("scout's tray row");
    assert_eq!(foot_of(cx, &view), (crate::subagents::Liveness::Running, tray), "scout, as its tray row says it");
    view.update(cx, |v, cx| {
        v.model.stats.subagents.append(&[
            serde_json::json!({"role": "assistant", "content": [{"type": "tool_use", "id": "toolu_new", "name": "subagent_start",
                "input": {"agent": "builder", "task": "build it"}}]}),
            serde_json::json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_new",
                "content": "{\"agent_name\":\"builder\",\"handle_id\":\"sa_1\",\"status\":\"running\"}"}]}),
        ]);
        assert_eq!(v.model.stats.subagents.ident(1).map(|(_, n)| n.to_string()).as_deref(), Some("builder"));
        cx.notify();
    });
    cx.run_until_parked();
    let own = view.read_with(cx, |v, _| v.model.watch.focused().map(|w| w.facts.to_string())).expect("scout's view");
    assert_eq!(foot_of(cx, &view), (crate::subagents::Liveness::Gone, own), "GONE, with scout's own facts");
    let (session, key) = view.read_with(cx, |v, _| {
        let w = v.model.watch.focused().expect("scout's view");
        (v.model.tabs.current_session().map(str::to_string).expect("its session"), (w.key().0.map(str::to_string), w.key().1.to_string()))
    });
    let tab = view.read_with(cx, |v, _| v.worker_state(&session, "sa_1", Some((key.0.as_deref(), &key.1))));
    assert_eq!(tab, Some(crate::subagents::Liveness::Gone), "its tab says the same");
}

/// Shady M2, the proper fix: a re-attach during a long tool call demotes
/// every running worker to started until it is heard again; its view's
/// next page (polled from the daemon, 4 a second) is heard by the
/// session's list, the one source: the foot, its worker tab and its tray
/// row say RUNNING, and READ ONLY or STEER WORKER follow from it.
#[gpui_kit::test]
fn a_reattach_leaves_a_running_worker_running(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    scout_in_place(cx, w, &view);
    view_running(cx, &view);
    view.update(cx, |v, cx| {
        v.model.stats.subagents.reattached();
        assert_eq!(v.model.stats.subagents.status(1), Some(crate::subagents::Status::Started), "demoted until heard again");
        cx.notify();
    });
    cx.run_until_parked();
    assert_eq!(foot_of(cx, &view).0, crate::subagents::Liveness::Started, "one source: started until heard");
    view_running(cx, &view);
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |v, _| v.model.stats.subagents.status(1)),
        Some(crate::subagents::Status::Running),
        "its page is heard by the list"
    );
    assert_eq!(foot_of(cx, &view).0, crate::subagents::Liveness::Running, "the foot: running, as its pages say");
    let tab = view.read_with(cx, |v, _| {
        let w = v.model.watch.focused().expect("scout's view");
        let session = v.model.tabs.current_session().expect("its session");
        v.worker_state(session, &w.handle_id, Some(w.key()))
    });
    assert_eq!(tab, Some(crate::subagents::Liveness::Running), "its tab: running too");
}

/// Shady N1: scout (sa_1) ended in a worker tab left in the background
/// (not polled: its view still says completed); a cold-started daemon
/// gave sa_1 to builder, running. Scout's stale view says nothing of
/// builder: its state is RUNNING (the tray row, its menu, its hover card),
/// and its menu's Cancel and its × cancel it.
#[gpui_kit::test]
fn a_stale_view_under_a_reused_handle_says_nothing_of_its_worker(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    let session = view.update(cx, |v, cx| {
        let r = v.tray.rows.iter().find(|r| r.id == 1).expect("scout's tray row");
        let scout = crate::wtabs::Who::new(1, r.name.clone(), r.call.clone());
        v.open_worker_tab(&scout, cx);
        assert_eq!(v.model.tabs.current_worker().map(|t| t.handle.as_str()), Some("sa_1"), "scout in a tab");
        let s = v.model.watch.focused_mut().expect("scout's view");
        s.gone = None;
        s.status = "completed".into();
        v.focus_tab(0);
        cx.notify();
        v.model.tabs.current_session().map(str::to_string).expect("the session")
    });
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        assert!(v.model.watch.focus().is_none(), "back on the session's thread");
        assert_eq!(v.model.watch.worker("sa_1").map(|w| w.state()), Some("completed"), "scout's view kept, ended");
    });
    view.update(cx, |v, cx| {
        v.model.stats.subagents.append(&[
            serde_json::json!({"role": "assistant", "content": [{"type": "tool_use", "id": "toolu_new", "name": "subagent_start",
                "input": {"agent": "builder", "task": "build it"}}]}),
            serde_json::json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_new",
                "content": "{\"agent_name\":\"builder\",\"handle_id\":\"sa_1\",\"status\":\"running\"}"}]}),
        ]);
        // Its live row: running (the history alone says started).
        let row = serde_json::json!({"ev": "subagent_rows", "rows": [{"subagent_id": 1, "agent_name": "builder", "status": "running",
            "cancel_requested": false, "elapsed_secs": 3.0, "finished_elapsed": null, "step": "cargo build", "tools": 1}]});
        v.model.stats.subagents.on_session(&synaps_client::protocol::decode_event(&row).expect("a rows event"));
        assert_eq!(v.model.stats.subagents.status(1), Some(crate::subagents::Status::Running), "the list: builder runs");
        // Scout's stale view is not heard as builder's.
        assert!(!v.model.heard_view("sa_1"), "a view of another worker feeds nothing");
        assert_eq!(v.model.stats.subagents.status(1), Some(crate::subagents::Status::Running));
        cx.notify();
    });
    cx.run_until_parked();
    let builder = view.read_with(cx, |v, _| {
        let (call, name) = v.model.stats.subagents.ident(1).expect("sa_1 named");
        assert_eq!(name, "builder");
        crate::wtabs::Who::new(1, name.to_string(), call.map(|c| c.to_string().into()))
    });
    let state = view.read_with(cx, |v, _| v.worker_state(&session, "sa_1", Some(builder.key())));
    assert_eq!(state, Some(crate::subagents::Liveness::Running), "builder runs, whatever scout's view said");
    // Its menu (and hover card) read the same: Cancel and Steer can act.
    cx.update_window(w, |_, window, cx| {
        view.update(cx, |v, cx| {
            let t = crate::wtabs::Target { session: session.clone(), who: builder.clone(), tab: None };
            v.open_menu(t, crate::menu::At::Point(gpui_kit::point(px(700.), px(500.))), false, window, cx);
        })
    })
    .expect("the window");
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        let (m, _) = v.menu.as_ref().expect("builder's menu");
        let cancel = m.items.iter().find(|i| i.act == Act::Cancel).expect("a Cancel item");
        assert_eq!(cancel.why_not, None, "Cancel can act: {}", cancel.label);
    });
    view.update(cx, |v, cx| {
        v.close_menu();
        v.press_cancel(&session, &builder, crate::menu::How::Click(1), cx);
    });
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        assert_eq!(v.watch_note, None, "the × was not refused");
        assert!(v.model.cancels.armed(&session, 1, builder.key(), std::time::Instant::now()), "its cancel armed");
    });
}

/// The other demo session in a tab, attached (with no client, the
/// synthetic transport answers its attach).
fn open_idle_cpu(cx: &mut TestAppContext, view: &Entity<AppView>, place: crate::tabs::Place) {
    view.update(cx, |v, cx| {
        assert!(v.client.is_none(), "the synthetic transport attaches");
        let other = crate::app::Open::Attach { session_id: crate::demo::IDLE_CPU.into(), fallback: None, open: false };
        v.open_tab(other, place);
        cx.notify();
    });
    cx.run_until_parked();
    assert_eq!(attached(cx, view).as_deref(), Some(crate::demo::IDLE_CPU), "the other session attached");
}

/// Shady L6: HALT's arm is for what was on screen. Another session's tab
/// (its turn running too) disarms it: the Esc there arms for that turn,
/// and never halts it.
#[gpui_kit::test]
fn a_halt_armed_in_one_session_never_halts_another(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    resolve_prompt(cx, &view);
    focus_composer(cx, w, &view);
    tap(cx, w, "escape");
    assert!(view.read_with(cx, |v, _| v.halts == 0 && v.halt_armed.is_some()), "armed");
    let client = view.update(cx, |v, _| v.client.take());
    open_idle_cpu(cx, &view, crate::tabs::Place::New);
    view.update(cx, |v, cx| {
        v.client = client;
        v.model.transcript.set_busy(true);
        cx.notify();
    });
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| v.halt_shown()), "the other session's turn runs, and this window drives it");
    assert!(view.read_with(cx, |v, _| v.halt_armed.is_none()), "the switch disarmed it");
    focus_composer(cx, w, &view);
    tap(cx, w, "escape");
    assert!(view.read_with(cx, |v, _| v.halts == 0 && v.halt_armed.is_some()), "the Esc there arms for its turn, no more");
    // Back to the first session's tab: disarmed again; its Esc halts nothing.
    view.update(cx, |v, cx| {
        v.focus_tab(0);
        cx.notify();
    });
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| v.halt_armed.is_none()), "the tab change disarmed it");
    focus_composer(cx, w, &view);
    tap(cx, w, "escape");
    assert_eq!(view.read_with(cx, |v, _| v.halts), 0, "no turn halted but by two Escs in one place");
}

/// Shady L2: a rename asked for another session whose attach never came
/// is forgotten once anything else is shown (or it waited too long): the
/// naming row never opens later, under someone's typing. Asked from the
/// session menu of its rail row, it opens once the attach lands.
#[gpui_kit::test]
fn a_rename_for_another_session_never_opens_later(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    let naming = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.model.named.naming);
    // Asked; its attach never landed; this session stayed on screen.
    view.update(cx, |v, cx| {
        v.rename_pending = Some((crate::demo::IDLE_CPU.into(), std::time::Instant::now()));
        cx.notify();
    });
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| v.rename_pending.is_none()), "forgotten: another session was on screen");
    let client = view.update(cx, |v, _| v.client.take());
    open_idle_cpu(cx, &view, crate::tabs::Place::New);
    assert!(!naming(cx), "that session opened later: no naming row");
    // On screen and attached, but asked too long ago: nothing.
    view.update(cx, |v, cx| {
        let long_ago = std::time::Instant::now().checked_sub(crate::app::RENAME_WAIT + std::time::Duration::from_secs(1));
        v.rename_pending = long_ago.map(|t| (crate::demo::IDLE_CPU.into(), t));
        cx.notify();
    });
    cx.run_until_parked();
    assert!(!naming(cx) && view.read_with(cx, |v, _| v.rename_pending.is_none()), "an expired rename opens nothing");
    // The session menu's Rename… on the first session's rail row: its
    // tab, then its naming row once it is attached.
    let first = view.read_with(cx, |v, _| v.model.tabs.iter().next().and_then(|t| t.session.clone())).expect("the first tab's session");
    let key = view
        .read_with(cx, |v, _| {
            v.rail.rail.rows.iter().find(|r| r.live.and_then(|i| v.model.sessions.get(i)).is_some_and(|m| m.id == first)).map(|r| r.key)
        })
        .expect("its rail row");
    cx.update_window(w, |_, window, cx| {
        view.update(cx, |v, cx| {
            v.open_session_menu(crate::app::SessionAt::Row(key), Default::default(), window, cx);
            v.session_menu_pick(crate::app::SessionAct::Rename, crate::menu::How::Key { press: true }, window, cx);
        })
    })
    .expect("the window");
    cx.run_until_parked();
    assert_eq!(attached(cx, &view), Some(first), "its session on screen");
    assert!(naming(cx), "its naming row opened");
    view.update(cx, |v, _| v.client = client);
}

/// Shady L1: the Esc that ends an IME composition is the composition's:
/// it arms nothing. The next Esc arms HALT, the one after halts.
#[gpui_kit::test]
fn an_esc_ending_a_composition_is_not_halts(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    resolve_prompt(cx, &view);
    focus_composer(cx, w, &view);
    cx.update_window(w, |_, window, cx| {
        view.update(cx, |v, cx| {
            v.composer.update(cx, |s, cx| gpui_kit::EntityInputHandler::replace_and_mark_text_in_range(s, None, "か", None, window, cx))
        })
    })
    .expect("the window");
    cx.run_until_parked();
    let composing = cx
        .update_window(w, |_, window, cx| {
            view.update(cx, |v, cx| v.composer.update(cx, |s, cx| gpui_kit::EntityInputHandler::marked_text_range(s, window, cx)))
        })
        .expect("the window");
    assert!(composing.is_some(), "a composition under way");
    tap(cx, w, "escape");
    assert!(view.read_with(cx, |v, _| v.halts == 0 && v.halt_armed.is_none()), "the composition's Esc arms nothing");
    tap(cx, w, "escape");
    assert!(view.read_with(cx, |v, _| v.halt_armed.is_some()), "the next arms");
    tap(cx, w, "escape");
    assert_eq!(view.read_with(cx, |v, _| v.halts), 1, "and the one after halts");
}

/// Finish gate F2: the foot's state word is the column's size (12·r):
/// at step 22 it is drawn larger than at 16, never the smallest text in
/// the foot.
#[gpui_kit::test]
fn the_foot_word_grows_with_the_text(cx: &mut TestAppContext) {
    let mut heights = Vec::new();
    for step in [16, 22] {
        let (win, _view, _events) = open_at(cx, (1280., 720.), step);
        let b = VisualTestContext::from_window(win, cx).debug_bounds("foot-word").expect("the word");
        heights.push(f32::from(b.size.height));
    }
    assert!(heights[1] > heights[0], "the word at 22 is taller than at 16: {heights:?}");
}

/// Shady L3: the rail's readout is not drawn under the session menu.
#[gpui_kit::test]
fn the_rail_readout_goes_under_the_session_menu(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    let key = view.read_with(cx, |v, _| v.rail.rail.rows.iter().find(|r| r.live.is_some()).map(|r| r.key)).expect("a live row");
    view.update(cx, |v, cx| {
        v.readout = Some(key);
        cx.notify();
    });
    cx.run_until_parked();
    cx.update_window(w, |_, window, cx| {
        view.update(cx, |v, cx| v.open_session_menu(crate::app::SessionAt::Row(key), Default::default(), window, cx))
    })
    .expect("the window");
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| v.smenu.is_some() && v.readout.is_none()), "the menu, and no readout under it");
}
