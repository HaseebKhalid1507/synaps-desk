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
    size: (f32, f32),
    text: u8,
) -> (AnyWindowHandle, Entity<AppView>, async_channel::Receiver<synaps_client::Event>) {
    open_with(cx, size, text, true)
}

/// [`open_at`], the beat on the layer (`layered`, the app's way) or in the
/// app's frame (`SYNAPS_DESKTOP_BEAT_IN_FRAME=1`).
fn open_with(
    cx: &mut TestAppContext,
    (w, h): (f32, f32),
    text: u8,
    layered: bool,
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
        crate::beat::open_window(options, cx, layered, |window, cx| {
            cx.new(|cx| {
                let v = AppView::new(None, Some(crate::demo::events()), window, cx);
                // The motion clock is the test's from the first frame: it
                // stands still until a test moves it ([`beat`],
                // [`fluid_frame`], `set_clock`), never with the runner's
                // wall time. On a loaded runner a draw took 110 ms: every
                // motion timer was already due when armed, `run_until_parked`
                // drew frame after frame (with the beat in the frame, never
                // parking), and a test step played hundreds of ms of motion.
                v.fx.pin_clock();
                v
            })
        })
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
    // The app has been up a second (the launch's column arrival and the
    // card's charge are over), as a fast runner's wall time used to make it.
    wait(cx, window, &view, 1000);
    (window, view, events)
}

/// `ms` go by on the test's clock: the motion clock and the executor's move
/// on together in one step (what fell due fires; animations are where they
/// are by then), and what that drew settles.
fn wait(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, ms: u64) {
    let d = std::time::Duration::from_millis(ms);
    view.read_with(cx, |v, _| v.fx.set_clock(v.fx.tick() + d));
    cx.executor().advance_clock(d);
    settle(cx, w, 8);
}

/// [`wait`] to just past `ms` in two steps, so a timer due at `ms` fires
/// on its own (a single jump lets the timers armed on the way chain past it).
fn wait_past(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, ms: u64) {
    wait(cx, w, view, ms - 1);
    wait(cx, w, view, 1);
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

/// Finish gate F1: ten tabs and more at 1920 (the rail at its 220 px
/// limit, the right pane at 413) sit at the strip's narrowest,
/// [`crate::tabstrip::TAB_MIN`]. Each
/// keeps its glyph, a 13-px title cut to at least a character and its
/// ellipsis, and its ×, none over another and all inside the tab, at text
/// steps 16 and 22. The test window advances every character 0.6 em.
#[cfg(debug_assertions)]
#[gpui_kit::test]
fn a_tab_at_its_narrowest_cuts_its_title_cleanly(cx: &mut TestAppContext) {
    use gpui_kit::VisualTestContext;

    use crate::tabstrip::TAB_MIN;
    let (w, view, _events) = open(cx);
    let tabs = view.update(cx, |v, cx| {
        v.client = None;
        // 1920 less the 220 rail and a 413 right pane: 8 tabs of 132 and
        // the arrows.
        v.panels.rail_width = Some(220);
        v.panels.right_width = Some(413);
        let ids: Vec<String> = v.model.sessions.iter().map(|s| s.id.clone()).collect();
        for id in ids {
            v.open_tab(crate::app::Open::Attach { session_id: id, fallback: None, open: false }, crate::tabs::Place::New);
        }
        cx.notify();
        v.model.tabs.iter().map(|t| (t.id, t.label.to_string())).collect::<Vec<_>>()
    });
    assert!(tabs.len() >= 10, "ten tabs and more: {}", tabs.len());
    let leak = |s: String| -> &'static str { Box::leak(s.into_boxed_str()) };
    let mut vcx = VisualTestContext::from_window(w, cx);
    for step in [16, 22] {
        view.update(&mut vcx, |v, cx| {
            v.show_text_size(step, cx);
            cx.notify();
        });
        vcx.simulate_resize(size(px(1920.), px(1080.)));
        vcx.run_until_parked();
        let mut shown = 0;
        for (id, label) in &tabs {
            let Some(tab) = vcx.debug_bounds(leak(format!("tab {id}"))) else { continue };
            shown += 1;
            let width = f32::from(tab.size.width);
            assert!((width - TAB_MIN).abs() < 0.5, "step {step}: tab {id} is {width} px");
            let mut part =
                |name: &str| vcx.debug_bounds(leak(format!("tab {id} {name}"))).unwrap_or_else(|| panic!("tab {id}: its {name}"));
            let (glyph, title, x) = (part("glyph"), part("title"), part("x"));
            let at = |v: gpui_kit::Pixels| f32::from(v);
            assert!(at(glyph.left()) >= at(tab.left()) && at(glyph.right()) <= at(title.left()), "step {step}: tab {id}'s glyph");
            assert!(at(title.right()) <= at(x.left()) + 0.01, "step {step}: tab {id}'s title runs under its ×");
            assert!(at(x.right()) <= at(tab.right()) + 0.01, "step {step}: tab {id}'s × past its edge");
            // A character and the ellipsis at the least; cut, not spilled.
            let cell = 0.6 * crate::fonts::size::TITLE;
            assert!(at(title.size.width) >= 2. * cell, "step {step}: tab {id}'s title has {:?}", title.size.width);
            if label.chars().count() as f32 * cell > at(title.size.width) {
                assert!(at(title.right()) <= at(tab.right()), "step {step}: tab {id}'s title cut inside it");
            }
        }
        assert!(shown >= 8, "step {step}: {shown} tabs drawn");
    }
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

/// Its view took a `running` page from the daemon (the offline demo's
/// view has no daemon to poll): the page, and what the pump does with it
/// (the pump's own path: `model::the_list_hears_only_the_pages_its_view_took`).
fn view_running(cx: &mut TestAppContext, view: &Entity<AppView>) {
    view.update(cx, |v, _| {
        let w = v.model.watch.focused_mut().expect("a worker on screen");
        w.gone = None;
        w.status = "running".into();
        let handle = w.handle_id.clone();
        v.model.heard_view(&handle, "running");
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
    let tray = view
        .read_with(cx, |v, _| v.tray.rows.iter().find(|r| r.id == 1).map(|r| crate::tray::ticked(&v.fx, &r.facts, &r.tick).to_string()))
        .expect("scout's tray row");
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

/// Shady #8 on the PR 4 rebase: a reconnect demotes a watched running
/// worker to started, and its view's next page lifts it (PR 3's one
/// source). Neither is news (motion.md §8: a reconnect is history): its
/// tray row's glyph and the foot's play no write-on, either way.
#[gpui_kit::test]
fn a_reconnect_replays_no_worker_write_ons(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    scout_in_place(cx, w, &view);
    view_running(cx, &view);
    settle(cx, w, 8);
    let shown = |cx: &mut TestAppContext| {
        view.read_with(cx, |v, _| v.tray.rows.iter().find(|r| r.id == 1).map(|r| r.live)).expect("scout's tray row")
    };
    assert_eq!(shown(cx), crate::subagents::Liveness::Running);
    let t0 = view.read_with(cx, |v, _| v.fx.clock());
    // The daemon attaches the same session again: a reconnect (its
    // attach alone: no row of scout's comes with it to lift it at once).
    view.update(cx, |v, cx| {
        let seq = v.model.active.as_ref().expect("attached").attach_seq;
        let events = crate::demo::attach(crate::demo::SID, seq)
            .into_iter()
            .filter_map(|e| match e {
                synaps_client::Event::Attached { attach_seq, attached, mode, .. } => {
                    Some(synaps_client::Event::Attached { attach_seq, attached, mode, reconnect: true })
                }
                _ => None,
            })
            .collect();
        v.apply_batch(events, cx);
    });
    settle(cx, w, 8);
    assert_eq!(foot_of(cx, &view).0, crate::subagents::Liveness::Started, "demoted until heard");
    assert_eq!(shown(cx), crate::subagents::Liveness::Started, "its row too");
    assert_eq!(in_place(cx, &view).as_deref(), Some("sa_1"), "still watched");
    view_running(cx, &view);
    view.update(cx, |_, cx| cx.notify());
    settle(cx, w, 8);
    assert_eq!(foot_of(cx, &view).0, crate::subagents::Liveness::Running, "lifted by its page");
    assert_eq!(shown(cx), crate::subagents::Liveness::Running);
    let replayed: Vec<_> = view
        .read_with(cx, |v, _| v.fx.changed_since(t0))
        .into_iter()
        .filter(|(t, _)| matches!(t, crate::motion::Tag::TrayGlyph | crate::motion::Tag::BlockGlyph | crate::motion::Tag::Foot))
        .collect();
    assert!(replayed.is_empty(), "no write-on replayed across the reconnect: {replayed:?}");
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
        assert!(!v.model.heard_view("sa_1", "running"), "a view of another worker feeds nothing");
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

/// A fan-out wider than its chips' lines (polish.md §2.5, finish gate
/// r2): the newest worker fails, and its chip is drawn, first, while
/// quiet running ones go into `+N more`; `+N more` opens the workers
/// menu, the workers not drawn, newest first; picked, its view in place.
#[gpui_kit::test]
fn a_failed_worker_is_never_folded_into_more(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    view.update(cx, |v, _| v.client = None);
    let rows = |failed: bool| {
        let rows: Vec<serde_json::Value> = (1..=12u64)
            .map(|n| {
                let status = if failed && n == 12 { "failed" } else { "running" };
                serde_json::json!({"subagent_id": n, "agent_name": format!("worker-{n}"), "status": status, "cancel_requested": false,
                    "elapsed_secs": 10.0 + n as f64, "finished_elapsed": null, "step": format!("reading src/part_{n}.rs"), "tools": n,
                    "handle_id": format!("sa_{n}")})
            })
            .collect();
        synaps_client::protocol::decode_event(&serde_json::json!({"ev": "subagent_rows", "rows": rows})).expect("a rows event")
    };
    view.update(cx, |v, cx| {
        v.model.stats.subagents.on_session(&rows(false));
        cx.notify();
    });
    settle(cx, w, 8);
    view.update(cx, |v, cx| {
        v.model.stats.subagents.on_session(&rows(true));
        cx.notify();
    });
    settle(cx, w, 8);
    let (ids, drawn, more) =
        view.read_with(cx, |v, _| (v.tray.rows.iter().map(|r| r.id).collect::<Vec<_>>(), v.tray_drawn(), v.chips.more));
    assert!(ids.contains(&12), "the failed one flashes in the tray: {ids:?}");
    assert!(more > 0 && drawn + more == ids.len(), "more workers than lines: {drawn} drawn, +{more}");
    assert_eq!(ids[0], 12, "the failure first: {ids:?}");
    let running: Vec<u64> = ids[1..].to_vec();
    assert!(running.windows(2).all(|p| p[0] > p[1]), "then the running, newest first: {running:?}");
    // `+N more` opens the workers menu: the ones not drawn.
    let at = VisualTestContext::from_window(w, cx).debug_bounds("tray-more").expect("+N more drawn").center();
    click_at(cx, w, at);
    let items = view.read_with(cx, |v, _| v.wmenu.as_ref().map(|m| m.items.iter().map(|i| i.act).collect::<Vec<_>>()));
    assert_eq!(items.as_deref(), Some(&ids[drawn..]), "the workers menu lists those not drawn");
    let pick = ids[drawn];
    cx.update_window(w, |_, window, cx| view.update(cx, |v, cx| v.workers_menu_pick(pick, crate::menu::How::Click(1), window, cx)))
        .expect("the window");
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        assert!(v.wmenu.is_none(), "picked: closed");
        assert_eq!(v.model.watch.focused().map(|w| w.subagent_id), Some(pick), "its view in place");
    });
}

/// The workers menu is a pointer menu like the others (fluid.md §3.10):
/// it enters from the click and leaves without a hitbox. While open it
/// says what `+N more` stands for now (a worker ended: its line and
/// place), its selection kept on the same worker. A reconnect closes it
/// (a restarted daemon numbers its workers anew: Shady #16 L-a, L-b).
#[gpui_kit::test]
fn the_workers_menu_follows_the_tray_and_closes_on_a_reconnect(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    view.update(cx, |v, _| v.client = None);
    let rows = |ended: Option<u64>| {
        let rows: Vec<serde_json::Value> = (1..=12u64)
            .map(|n| {
                let status = if ended == Some(n) { "completed" } else { "running" };
                serde_json::json!({"subagent_id": n, "agent_name": format!("worker-{n}"), "status": status, "cancel_requested": false,
                    "elapsed_secs": 10.0 + n as f64, "finished_elapsed": if ended == Some(n) { serde_json::json!(20.0) } else { serde_json::Value::Null },
                    "step": format!("reading src/part_{n}.rs"), "tools": n, "handle_id": format!("sa_{n}")})
            })
            .collect();
        synaps_client::protocol::decode_event(&serde_json::json!({"ev": "subagent_rows", "rows": rows})).expect("a rows event")
    };
    view.update(cx, |v, cx| {
        v.model.stats.subagents.on_session(&rows(None));
        cx.notify();
    });
    settle(cx, w, 8);
    let at = VisualTestContext::from_window(w, cx).debug_bounds("tray-more").expect("+N more drawn").center();
    click_at(cx, w, at);
    let (born, items) = view.read_with(cx, |v, _| {
        let m = v.wmenu.as_ref().expect("the workers menu");
        (m.born, m.items.iter().map(|i| i.act).collect::<Vec<_>>())
    });
    assert!(born.is_some(), "the pointer opened it: it enters");
    assert!(items.len() >= 3, "workers not drawn: {items:?}");
    // The second item selected; the first one listed ends.
    let (gone, kept) = (items[0], items[1]);
    view.update(cx, |v, cx| {
        v.wmenu.as_mut().expect("open").sel = Some(1);
        v.model.stats.subagents.on_session(&rows(Some(gone)));
        cx.notify();
    });
    settle(cx, w, 8);
    view.read_with(cx, |v, _| {
        let m = v.wmenu.as_ref().expect("still open");
        let now: Vec<u64> = m.items.iter().map(|i| i.act).collect();
        let expect: Vec<u64> = v.tray.rows[v.tray_drawn()..].iter().map(|r| r.id).collect();
        assert_eq!(now, expect, "what +N more stands for now");
        assert_ne!(now, items, "the ended one moved (ended last)");
        assert_eq!(m.sel.map(|i| m.items[i].act), Some(kept), "the selection on the same worker");
        let line = &m.items[now.iter().position(|&a| a == gone).expect("the ended one listed")].label;
        assert!(line.contains(crate::glyph::worker(crate::subagents::Liveness::Done).2), "its line says it ended: {line}");
    });
    // A reconnect closes it; it leaves, then is gone.
    view.update(cx, |v, cx| {
        let seq = v.model.active.as_ref().expect("attached").attach_seq;
        let events = crate::demo::attach(crate::demo::SID, seq)
            .into_iter()
            .filter_map(|e| match e {
                synaps_client::Event::Attached { attach_seq, attached, mode, .. } => {
                    Some(synaps_client::Event::Attached { attach_seq, attached, mode, reconnect: true })
                }
                _ => None,
            })
            .collect();
        v.apply_batch(events, cx);
    });
    wait(cx, w, &view, 7);
    view.read_with(cx, |v, _| {
        assert!(v.wmenu.is_none(), "closed by the reconnect");
        assert!(v.wmenu_leaving.is_some(), "leaving");
    });
    for _ in 0..30 {
        wait(cx, w, &view, 7);
    }
    assert!(view.read_with(cx, |v, _| v.wmenu_leaving.is_none()), "gone");
}

/// Shady #15 L1: a workers menu emptied in the render (the window grew,
/// every worker got its chip, the tray's key unchanged) leaves from that
/// frame, at its place, and is gone after its exit; not later, when
/// something else draws.
#[gpui_kit::test]
fn a_workers_menu_emptied_by_room_leaves_at_once(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    view.update(cx, |v, _| v.client = None);
    let rows: Vec<serde_json::Value> = (1..=6u64)
        .map(|n| {
            serde_json::json!({"subagent_id": n, "agent_name": format!("worker-{n}"), "status": "running", "cancel_requested": false,
                "elapsed_secs": 10.0 + n as f64, "step": format!("reading src/part_{n}.rs"), "tools": n, "handle_id": format!("sa_{n}")})
        })
        .collect();
    let ev = synaps_client::protocol::decode_event(&serde_json::json!({"ev": "subagent_rows", "rows": rows})).expect("a rows event");
    view.update(cx, |v, cx| {
        v.model.stats.subagents.on_session(&ev);
        cx.notify();
    });
    VisualTestContext::from_window(w, cx).simulate_resize(size(px(1440.), px(560.)));
    settle(cx, w, 8);
    let drawn = view.read_with(cx, |v, _| (v.tray_drawn(), v.tray.rows.len()));
    let at = VisualTestContext::from_window(w, cx).debug_bounds("tray-more").unwrap_or_else(|| panic!("+N more drawn: {drawn:?}")).center();
    click_at(cx, w, at);
    let key = view.read_with(cx, |v, _| {
        assert!(v.wmenu.as_ref().expect("the workers menu").born.is_some(), "the pointer opened it");
        v.tray.key
    });
    for _ in 0..20 {
        wait(cx, w, &view, 7);
    }
    // Frame by frame: (open, leaving) after each.
    let state = |cx: &mut TestAppContext| view.read_with(cx, |v, _| (v.wmenu.is_some(), v.wmenu_leaving.is_some()));
    VisualTestContext::from_window(w, cx).simulate_resize(size(px(1440.), px(900.)));
    let mut seen = vec![state(cx)];
    for _ in 0..8 {
        cx.update_window(w, |_, window, cx| window.simulate_next_frame(cx)).expect("the window");
        cx.run_until_parked();
        seen.push(state(cx));
    }
    let closed = seen.iter().position(|s| !s.0).unwrap_or_else(|| panic!("closed: {seen:?}"));
    assert!(seen[closed].1, "leaving from the frame it closed in: {seen:?}");
    view.read_with(cx, |v, _| {
        assert_eq!(v.tray.key, key, "the tray's key unchanged: only the render sees the room");
        assert_eq!(v.tray_drawn(), v.tray.rows.len(), "every worker has its chip");
    });
    for _ in 0..30 {
        wait(cx, w, &view, 7);
    }
    view.read_with(cx, |v, _| assert!(v.wmenu_leaving.is_none() && v.wmenu_was.is_none(), "gone, nothing left to leave later"));
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

/// Frames with nothing changed (the pointer over the rail draws them, a
/// row's hover the only difference): every markdown row keeps the text
/// view it had (none dropped from the bounded cache, none made again and
/// laid out a frame late), and none is stale. The text dropout seen in
/// x11grab captures (docs/bench/text-dropout/README.md) was the capture
/// reading a frame before the GPU had finished it; the frame the app hands
/// the renderer is the same from one redraw to the next, and this keeps
/// the app's half of that.
#[gpui_kit::test]
fn a_redraw_with_nothing_changed_keeps_every_text_view(cx: &mut TestAppContext) {
    let (_w, view, _events) = open_at(cx, (1920., 1080.), 16);
    let state = |cx: &mut TestAppContext| {
        view.read_with(cx, |v, _| {
            let t = v.texts.borrow();
            (t.frames(), t.entities())
        })
    };
    let (first, kept) = state(cx);
    assert!(!kept.is_empty(), "the demo's markdown rows drawn, their views kept");
    for _ in 0..40 {
        view.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
    }
    let (last, again) = state(cx);
    assert!(last >= first + 40, "each notify drew a frame: {first} then {last}");
    assert_eq!(again, kept, "no text view dropped or made again across 40 frames");
    assert_eq!(view.read_with(cx, |v, _| v.text_views().1), 0, "none stale");
}

// ── motion (cohesion/motion.md §10, PR 4) ─────────────────────────────

/// Delivers next frames until the window asks for none (at most `max`):
/// how many it drew.
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

fn set_motion(cx: &mut TestAppContext, view: &Entity<AppView>, level: crate::motion::Level) {
    cx.update(|cx| {
        cx.set_global(crate::motion::Setting { level: Some(level), lock: None });
        crate::motion::Level::apply(cx);
    });
    view.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
}

/// What the window's motion asked for after its last frame.
fn motion_pending(cx: &mut TestAppContext, view: &Entity<AppView>) -> (bool, Option<std::time::Instant>) {
    view.read_with(cx, |v, _| v.fx.pending())
}

/// Draws one more frame now.
fn frame(cx: &mut TestAppContext, view: &Entity<AppView>) {
    view.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
}

/// A run of the other demo session folded, then its id (it has three).
fn a_folded_run(cx: &mut TestAppContext, view: &Entity<AppView>) -> u64 {
    view.read_with(cx, |v, _| {
        v.disp.iter().find(|d| matches!(d.kind, crate::turns::Kind::Summary { open: false, .. })).map(|d| d.id).expect("a folded run")
    })
}

/// Every row on screen, by identity: its top (the gate's "column").
fn row_tops(cx: &mut TestAppContext, view: &Entity<AppView>) -> std::collections::HashMap<(u64, bool), f32> {
    view.read_with(cx, |v, _| {
        (0..v.list.item_count())
            .filter_map(|ix| {
                let d = v.disp.get(ix)?;
                let b = v.list.bounds_for_item(ix)?;
                let vb = v.list.viewport_bounds();
                (b.bottom() > vb.top() && b.top() < vb.bottom())
                    .then(|| ((d.id, matches!(d.kind, crate::turns::Kind::Summary { .. })), f32::from(b.top())))
            })
            .collect()
    })
}

/// Frames until no fold room is left: the largest move of a row on screen
/// in two frames running, and how many frames had the room.
fn fold_frames(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) -> (f32, usize) {
    let mut last = row_tops(cx, view);
    let (mut step, mut gapped) = (0f32, 0);
    for f in 0..600 {
        fluid_frame(cx, w, view);
        let now = row_tops(cx, view);
        let moved: Vec<((u64, bool), f32, f32)> = now.iter().filter_map(|(k, y)| last.get(k).map(|y0| (*k, *y, y - y0))).collect();
        let big = moved.iter().map(|m| m.2.abs()).fold(0f32, f32::max);
        if big > 17. {
            eprintln!("fold_frames: frame {f}: rows (key, top, moved) {moved:?}");
        }
        step = step.max(big);
        last = now;
        if VisualTestContext::from_window(w, cx).debug_bounds("fold-gap").is_some() {
            gapped += 1;
        } else if gapped > 0 {
            break;
        }
    }
    (step, gapped)
}

/// A turn of `n` finished calls at the list's end, then its end: the run
/// folds itself.
fn turn_of_calls(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, n: usize) {
    use synaps_client::protocol::{LlmEvent, StreamEvent};
    view.update(cx, |v, _| v.model.transcript.set_busy(true));
    running_tools(cx, view, n);
    view.update(cx, |v, cx| {
        for k in 0..n {
            v.model.transcript.apply(&SessionEvent::Stream {
                event: StreamEvent::Llm(LlmEvent::ToolResult { tool_id: format!("ui-run-{k}"), result: "ok".into() }),
            });
        }
        v.sync_list();
        v.list.scroll_to_end();
        cx.notify();
    });
    for _ in 0..240 {
        fluid_frame(cx, w, view);
    }
}

/// The design gate of #23, N1: a run folds on a spring, never a cut. No
/// row on screen moves more than about 40 px a 60 Hz frame (17 px a 144 Hz
/// test frame), and the fold takes well over 15 60 Hz frames:
/// - clicked mid-screen, its header holds (the list keeps its top row);
/// - clicked at the list's end, its header holds too (the room is held);
/// - at a turn's end, at the list's end, with its header on screen and with
///   it far above the view (a long run): the end holds, what is above
///   comes down.
///
/// Reduced and Off: a cut, the header still where it was.
#[gpui_kit::test]
fn a_run_folds_on_a_spring_never_a_cut(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    view.update(cx, |v, _| v.client = None);
    open_idle_cpu(cx, &view, crate::tabs::Place::Current);
    settle(cx, w, 8);
    let run = a_folded_run(cx, &view);
    let header = |cx: &mut TestAppContext| {
        VisualTestContext::from_window(w, cx).debug_bounds(String::leak(format!("run-summary {run}"))).map(|b| f32::from(b.top()))
    };
    let toggle = |cx: &mut TestAppContext| {
        view.update(cx, |v, cx| {
            v.toggle_run(run);
            cx.notify();
        });
        cx.run_until_parked();
    };
    let mid = |cx: &mut TestAppContext| {
        view.update(cx, |v, cx| {
            let ix = v.disp.iter().position(|d| d.id == run).expect("its header");
            v.list.scroll_to(gpui::ListOffset { item_ix: ix, offset_in_item: gpui_kit::px(0.) });
            v.list.scroll_by(gpui_kit::px(-120.));
            cx.notify();
        });
        for _ in 0..4 {
            fluid_frame(cx, w, &view);
        }
    };
    // Clicked mid-screen.
    toggle(cx);
    for _ in 0..200 {
        fluid_frame(cx, w, &view);
    }
    mid(cx);
    let at = header(cx).expect("its header, unfolded mid-screen");
    toggle(cx);
    assert_eq!(header(cx), Some(at), "the click: the header holds");
    let (step, gapped) = fold_frames(cx, w, &view);
    assert_eq!(header(cx), Some(at), "landed: the header where it was");
    assert!(step <= 17., "mid-screen: a row moved {step} px in one frame");
    assert!(gapped >= 36, "mid-screen: {gapped} 144 Hz frames");
    // The turn's end at the list's end: its header on screen, then far
    // above it.
    for n in [5, 25] {
        turn_of_calls(cx, w, &view, n);
        let tops = row_tops(cx, &view);
        view.update(cx, |v, cx| {
            v.model.transcript.apply(&SessionEvent::Idle);
            v.sync_list();
            cx.notify();
        });
        cx.run_until_parked();
        assert!(VisualTestContext::from_window(w, cx).debug_bounds("fold-gap").is_some(), "{n} calls: the room held, not cut");
        let (step, gapped) = fold_frames(cx, w, &view);
        assert!(step <= 17., "{n} calls at the turn's end: a row moved {step} px in one frame");
        assert!(gapped >= 20, "{n} calls at the turn's end: {gapped} frames");
        assert!(!tops.is_empty());
        if n == 5 {
            // Clicked at the list's end: unfolded, then folded; its header
            // holds (the room past the end is held).
            let last =
                view.read_with(cx, |v, _| v.disp.iter().rev().find(|d| matches!(d.kind, crate::turns::Kind::Summary { .. })).map(|d| d.id));
            let last = last.expect("the turn's run");
            let flip = |cx: &mut TestAppContext| {
                view.update(cx, |v, cx| {
                    v.toggle_run(last);
                    cx.notify();
                });
                cx.run_until_parked();
            };
            flip(cx);
            for _ in 0..240 {
                fluid_frame(cx, w, &view);
            }
            view.update(cx, |v, cx| {
                v.list.scroll_to_end();
                cx.notify();
            });
            for _ in 0..4 {
                fluid_frame(cx, w, &view);
            }
            let sel = String::leak(format!("run-summary {last}"));
            let at = VisualTestContext::from_window(w, cx).debug_bounds(sel).map(|b| f32::from(b.top())).expect("its header at the end");
            flip(cx);
            let (step, gapped) = fold_frames(cx, w, &view);
            let now = VisualTestContext::from_window(w, cx).debug_bounds(sel).map(|b| f32::from(b.top()));
            assert_eq!(now, Some(at), "clicked at the end: the header holds");
            assert!(step <= 17., "clicked at the end: a row moved {step} px in one frame");
            assert!(gapped >= 20, "clicked at the end: {gapped} frames");
        }
    }
    // Reduced and Off: a cut, the header where it was.
    for level in [crate::motion::Level::Reduced, crate::motion::Level::Off] {
        set_motion(cx, &view, level);
        settle(cx, w, 4);
        toggle(cx);
        settle(cx, w, 4);
        mid(cx);
        let at = header(cx);
        toggle(cx);
        settle(cx, w, 2);
        assert!(VisualTestContext::from_window(w, cx).debug_bounds("fold-gap").is_none(), "{level:?}: no room");
        assert_eq!(header(cx), at, "{level:?}: the header where it was");
    }
}

/// Off is the static app: whatever happens (a worker running, a send, a
/// worker's view in and out, the card answered, a session switch, a fold
/// and an unfold), motion asks for no frame and arms no timer: the guard is
/// `motion_pending() == (false, None)` after every event. The frame counts
/// (`settle`) are the static app's own: up to 2 follow-up frames after an
/// event (its hint canvases measure on the next frame), as on main; motion
/// adds none (0 frames of its own per event).
#[gpui_kit::test]
fn off_is_static(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Off);
    assert!(settle(cx, w, 8) <= 2);
    assert_eq!(motion_pending(cx, &view), (false, None), "scout runs in the tray: nothing scans under Off");
    // A worker's view in, and out.
    scout_in_place(cx, w, &view);
    assert!(settle(cx, w, 8) <= 2);
    assert_eq!(motion_pending(cx, &view), (false, None), "no bar draw opening a worker's view under Off");
    view.update(cx, |v, cx| v.leave_worker(false, cx));
    cx.run_until_parked();
    assert!(settle(cx, w, 8) <= 2);
    assert_eq!(motion_pending(cx, &view), (false, None));
    // A send's Handoff (what `submit` stamps).
    view.update(cx, |v, cx| {
        v.fx.sent.set(Some((v.fx.clock(), false)));
        cx.notify();
    });
    cx.run_until_parked();
    assert!(settle(cx, w, 8) <= 2);
    assert_eq!(motion_pending(cx, &view), (false, None), "no Handoff under Off");
    // The card answered: it goes in the same frame.
    resolve_prompt(cx, &view);
    assert!(settle(cx, w, 8) <= 2);
    assert_eq!(motion_pending(cx, &view), (false, None), "no exit fade under Off");
    assert!(view.read_with(cx, |v, _| v.card_exit.is_none()));
    view.update(cx, |v, _| v.client = None);
    open_idle_cpu(cx, &view, crate::tabs::Place::Current);
    assert!(settle(cx, w, 8) <= 2);
    let run = a_folded_run(cx, &view);
    view.update(cx, |v, cx| {
        v.toggle_run(run);
        cx.notify();
    });
    cx.run_until_parked();
    assert!(settle(cx, w, 8) <= 2, "the unfold's own frame, and the static app's hint");
    assert_eq!(motion_pending(cx, &view), (false, None), "no cascade under Off");
    view.update(cx, |v, cx| {
        v.toggle_run(run);
        cx.notify();
    });
    cx.run_until_parked();
    assert!(settle(cx, w, 8) <= 2);
    assert_eq!(motion_pending(cx, &view), (false, None), "no tally draw under Off");
    cx.update(|cx| assert!(cx.reduce_motion(), "the kit's own motion is off too"));
}

/// Nothing running on screen, nothing moving: after the last animation the
/// window asks for no frame and arms no timer.
#[gpui_kit::test]
fn idle_requests_no_frames(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    view.update(cx, |v, _| v.client = None);
    open_idle_cpu(cx, &view, crate::tabs::Place::Current);
    let run = a_folded_run(cx, &view);
    view.update(cx, |v, cx| {
        v.toggle_run(run);
        cx.notify();
    });
    cx.run_until_parked();
    // The cascade runs on display frames, then stops by itself.
    let mut drawn = 0;
    while motion_pending(cx, &view).0 && drawn < 400 {
        fluid_frame(cx, w, &view);
        drawn += 1;
    }
    assert!(drawn > 0, "the unfold animated");
    assert!(settle(cx, w, 8) <= 2);
    // The switch's new numbers afterglow (motion.md §6.6, fluid.md §3.6):
    // three steps on a timer over `glow`.
    wait(cx, w, &view, crate::motion::GLOW.as_millis() as u64);
    only_the_rail_spins(cx, w, &view, "settled");
}

/// Cards D × Q5 (#23 merged with #21): the rail's lattices and its fold
/// never fight. Folding and unfolding the rail on `sheet` asks for the
/// same display frames at the same widths whether its sessions run (their
/// lattices on the beat layer, clipped with the flight's box) or rest: the
/// beat adds no frame to the spring. Folded, the strip draws no lattice:
/// nothing on the layer, no frame, no timer, though the sessions still run.
/// Unfolded, their lattices are back on the beat.
#[gpui_kit::test]
fn the_rails_lattices_and_its_fold_never_fight(cx: &mut TestAppContext) {
    use crate::motion::Beat;
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    view.update(cx, |v, _| v.client = None);
    open_idle_cpu(cx, &view, crate::tabs::Place::Current);
    settle(cx, w, 8);
    wait(cx, w, &view, crate::motion::GLOW.as_millis() as u64);
    let spins =
        |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.fx.sites().iter().filter(|s| matches!(s.beat, Beat::Spin(_))).count());
    let busy = |cx: &mut TestAppContext, on: bool| {
        view.update(cx, |v, cx| {
            for (k, m) in v.model.sessions.iter_mut().enumerate() {
                (m.busy, m.subagents_running) = (on && k < 3, 0);
            }
            v.model.rail_gen += 1;
            cx.notify();
        });
        cx.run_until_parked();
        settle(cx, w, 8);
    };
    // A fold or an unfold, frame by frame: whether a display frame was
    // asked for before each, and the width it drew.
    let fly = |cx: &mut TestAppContext| {
        view.update(cx, |v, cx| v.toggle_panel(true, cx));
        cx.run_until_parked();
        let mut steps = Vec::new();
        for _ in 0..400 {
            let (shown, _, moving) = panel(cx, &view, true);
            if !moving {
                break;
            }
            steps.push((motion_pending(cx, &view).0, shown.round() as i32));
            fluid_frame(cx, w, &view);
        }
        assert!(steps.len() > 10, "it flew: {steps:?}");
        steps
    };
    busy(cx, true);
    assert!(spins(cx) > 0, "the rail's running sessions: lattices on the layer");
    let busy_fold = fly(cx);
    settle(cx, w, 8);
    assert_eq!(spins(cx), 0, "folded: the strip draws no lattice");
    for _ in 0..4 {
        wait(cx, w, &view, 500);
    }
    assert_eq!(motion_pending(cx, &view), (false, None), "folded, its sessions running: no frame, no timer");
    let busy_unfold = fly(cx);
    settle(cx, w, 8);
    assert!(spins(cx) > 0, "unfolded: they're back");
    let (_, timer) = motion_pending(cx, &view);
    let at = timer.expect("on the beat");
    assert!(view.read_with(cx, |v, _| v.fx.on_beats(at, 1)), "the timer is the beat's");
    busy(cx, false);
    assert_eq!(spins(cx), 0, "at rest");
    let rest_fold = fly(cx);
    settle(cx, w, 8);
    let rest_unfold = fly(cx);
    assert_eq!(busy_fold, rest_fold, "the fold: the same frames, running or not");
    assert_eq!(busy_unfold, rest_unfold, "the unfold: the same frames, running or not");
}

/// Nothing runs in the column, the composer or the tray (Shady #17 M3,
/// pinned): no display frame; no orbit; the only sites are the rail's
/// running sessions' spinners (polish.md P1); and the only timer, if any,
/// is theirs: the beat layer's, on the beat grid. Then, the rail's
/// sessions at rest too, gate 3 exactly: no frame and no timer.
fn only_the_rail_spins(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, what: &str) {
    let (frame, timer) = motion_pending(cx, view);
    assert!(!frame, "{what}: a display frame");
    let (sites, rail) = view.read_with(cx, |v, _| (v.fx.sites(), v.rail_width));
    let cards = sites.iter().filter(|s| matches!(s.beat, crate::motion::Beat::Lattice(_))).count();
    assert_eq!(cards, 0, "{what}: a card's or the composer's lattice");
    assert!(sites.iter().all(|s| matches!(s.beat, crate::motion::Beat::Spin(_)) && f32::from(s.origin.x) < rail), "{what}: {sites:?}");
    let at = timer.expect("the demo's rail runs: its spinners' beat");
    assert!(!sites.is_empty(), "{what}: a timer with nothing on the layer");
    assert!(view.read_with(cx, |v, _| v.fx.on_beats(at, 1)), "{what}: the timer is the beat's, not 30 Hz or a frame's");
    assert_eq!(view.read_with(cx, |v, _| v.fx.wakes()), Some(crate::motion::Wake::Beat), "{what}: the layer's beat, not the app");
    view.update(cx, |v, cx| {
        for m in v.model.sessions.iter_mut() {
            (m.busy, m.subagents_running) = (false, 0);
        }
        v.model.rail_gen += 1;
        cx.notify();
    });
    for _ in 0..4 {
        wait(cx, w, view, 500);
    }
    assert_eq!(view.read_with(cx, |v, _| v.fx.sites().len()), 0, "{what}, the rail at rest: nothing on the layer");
    assert_eq!(motion_pending(cx, view), (false, None), "{what}, the rail at rest: no frame, no timer");
}

/// A screen full of running things: no display frames for ambient, one
/// timer to the next beat; every glyph samples the one beat.
#[gpui_kit::test]
fn heartbeat_is_one_timer(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    // A second goes by: the card's AUTHORIZE charge (its own frames) ends.
    for _ in 0..8 {
        beat(cx, w, &view);
    }
    let (display, timer) = motion_pending(cx, &view);
    assert!(!display, "ambient asks for no display frames");
    let at = timer.expect("scout runs: the heartbeat is armed");
    // The instant the timer was armed from: the last frame's (Shady M4: not
    // the clock, which a slow runner's wall time has moved on since).
    let now = view.read_with(cx, |v, _| v.fx.frame_now());
    assert!(at > now - std::time::Duration::from_millis(1));
    let clock = view.read_with(cx, |v, _| v.fx.clock());
    assert!(at <= clock + crate::motion::BEAT, "to the next beat");
    let arms = view.read_with(cx, |v, _| v.fx.arms.get());
    frame(cx, &view);
    let again = view.read_with(cx, |v, _| v.fx.arms.get());
    assert!(again - arms <= 1, "one timer per beat, however many glyphs scan");
}

/// Hidden: the heartbeat's timer goes; shown again, the next frame arms it.
#[gpui_kit::test]
fn heartbeat_stops_hidden(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    // A second goes by (the card's AUTHORIZE charge, display frames until
    // it ends, arms no timer): on the test's clock, not the runner's.
    for _ in 0..8 {
        beat(cx, w, &view);
    }
    assert!(motion_pending(cx, &view).1.is_some(), "scout runs: the heartbeat is armed");
    cx.simulate_window_visibility_change(w, gpui_kit::WindowVisibility::Hidden);
    cx.run_until_parked();
    assert_eq!(motion_pending(cx, &view).1, None, "hidden: no timer");
    frame(cx, &view);
    assert_eq!(motion_pending(cx, &view).1, None, "a frame while hidden arms nothing");
    cx.simulate_window_visibility_change(w, gpui_kit::WindowVisibility::Visible);
    cx.run_until_parked();
    assert!(motion_pending(cx, &view).1.is_some(), "shown: armed again");
}

/// An unfold's cascade (bars drawing, the chevron turning, the opened
/// rows revealing below the header: fluid.md §3.3) never moves a row that
/// was there: every row on screen before it has one height on every frame.
#[gpui_kit::test]
fn transcript_heights_stable_across_frames(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    view.update(cx, |v, _| v.client = None);
    open_idle_cpu(cx, &view, crate::tabs::Place::Current);
    settle(cx, w, 8);
    let run = a_folded_run(cx, &view);
    let before: std::collections::HashSet<u64> = view.read_with(cx, |v, _| v.disp.iter().map(|d| d.id).collect());
    view.update(cx, |v, cx| {
        v.toggle_run(run);
        cx.notify();
    });
    cx.run_until_parked();
    assert!(motion_pending(cx, &view).0, "the cascade is running");
    let heights = |cx: &mut TestAppContext| {
        view.read_with(cx, |v, _| {
            (0..v.disp.len())
                .filter_map(|ix| v.list.bounds_for_item(ix).map(|b| (v.disp[ix].id, f32::from(b.size.height))))
                .collect::<Vec<_>>()
        })
    };
    let first: Vec<(u64, f32)> = heights(cx).into_iter().filter(|(id, _)| before.contains(id)).collect();
    assert!(first.len() > 3, "rows on screen: {first:?}");
    let mut frames = 0;
    while motion_pending(cx, &view).0 && frames < 60 {
        fluid_frame(cx, w, &view);
        frames += 1;
        let now = heights(cx);
        for (id, h) in &now {
            if let Some((_, h0)) = first.iter().find(|(i, _)| i == id) {
                assert_eq!(h, h0, "row {id} changed height on frame {frames}");
            }
        }
    }
    assert!(frames > 0, "the cascade ran over frames ({frames})");
    // Every row, the user blocks with it (their bar is an absolute child
    // now), is as high under Full as under Off, the static app.
    let full = heights(cx);
    assert!(view.read_with(cx, |v, _| v.disp.iter().any(|r| matches!(r.kind, crate::turns::Kind::User))), "user blocks on screen");
    set_motion(cx, &view, crate::motion::Level::Off);
    settle(cx, w, 8);
    assert_eq!(heights(cx), full, "Off and Full lay the column out alike");
}

/// The clicked header is the anchor (fluid.md §3.3): a run unfolded under
/// Full at the end of the list (tail-follow on) reveals its rows below its
/// header, which is drawn on every frame, at its height and its y, until
/// they have. Its id is its first row's: that row's reveal is not its.
#[gpui_kit::test]
fn an_unfold_under_full_keeps_its_header_put(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    view.update(cx, |v, _| v.client = None);
    open_idle_cpu(cx, &view, crate::tabs::Place::Current);
    // The column's glide in (a session opened) ends first.
    let mut n = 0;
    while motion_pending(cx, &view).0 && n < 400 {
        fluid_frame(cx, w, &view);
        n += 1;
    }
    settle(cx, w, 8);
    assert!(view.read_with(cx, |v, _| v.list.is_following_tail()), "at the end, following");
    let header = |cx: &mut TestAppContext, run: u64| {
        view.read_with(cx, |v, _| {
            let ix = v.disp.iter().position(|d| d.id == run && matches!(d.kind, crate::turns::Kind::Summary { .. }))?;
            v.list.bounds_for_item(ix).map(|b| (f32::from(b.origin.y), f32::from(b.size.height)))
        })
    };
    // The last folded run on screen: the one the clip clicks.
    let run = view.read_with(cx, |v, _| {
        v.disp
            .iter()
            .enumerate()
            .filter(|(ix, d)| matches!(d.kind, crate::turns::Kind::Summary { open: false, .. }) && v.list.bounds_for_item(*ix).is_some())
            .map(|(_, d)| d.id)
            .last()
            .expect("a folded run on screen")
    });
    let at = header(cx, run).expect("drawn");
    assert!(at.1 > 0., "{at:?}");
    view.update(cx, |v, cx| {
        v.toggle_run(run);
        cx.notify();
    });
    cx.run_until_parked();
    assert!(motion_pending(cx, &view).0, "the cascade is running");
    let mut frames = 0;
    while motion_pending(cx, &view).0 && frames < 120 {
        fluid_frame(cx, w, &view);
        frames += 1;
        assert_eq!(header(cx, run), Some(at), "the header on frame {frames}");
    }
    assert!(frames > 2, "the cascade ran over frames ({frames})");
    let opened = view.read_with(cx, |v, _| v.disp.iter().filter(|d| d.kind == crate::turns::Kind::Work).count());
    assert!(opened >= 5, "its rows are in the list ({opened})");
    settle(cx, w, 8);
    assert_eq!(header(cx, run), Some(at), "at rest");
}

/// A fold during its own unfold's reveal takes its rows out of the hold
/// at once (Shady #17 L3): the hold never keeps ids no longer listed.
#[gpui_kit::test]
fn a_fold_drops_its_rows_from_the_unfold_hold(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    view.update(cx, |v, _| v.client = None);
    open_idle_cpu(cx, &view, crate::tabs::Place::Current);
    settle(cx, w, 8);
    let run = view.read_with(cx, |v, _| {
        v.disp
            .iter()
            .filter(|d| matches!(d.kind, crate::turns::Kind::Summary { open: false, .. }))
            .map(|d| d.id)
            .next_back()
            .expect("a folded run")
    });
    view.update(cx, |v, cx| {
        v.toggle_run(run);
        cx.notify();
    });
    let held = view.read_with(cx, |v, _| v.unfold_hold.len());
    assert!(held > 0, "the unfold's rows are held while they reveal");
    view.update(cx, |v, cx| {
        v.toggle_run(run);
        cx.notify();
    });
    let left = view.read_with(cx, |v, _| {
        let shown: std::collections::HashSet<u64> = v.disp.iter().map(|d| d.id).collect();
        v.unfold_hold.iter().filter(|id| !shown.contains(id)).count()
    });
    assert_eq!(left, 0, "folded: none of its rows stay held");
    assert!(view.read_with(cx, |v, _| v.unfold_hold.is_empty()), "nothing else was held");
}

/// Words arrive readable (fluid.md law 3, §3.4): the one stream fade in
/// the app is `motion::text_motion` (its words start at 70 %), given only
/// to text streamed into a row (`turns.rs`); no view gets the kit's own
/// policy (`.motion(`), and no other file makes one. A lint over every
/// source file: the text run's colour is not observable from a test window.
#[test]
fn one_stream_fade_policy() {
    assert_eq!(gpui_kit::base::TextViewMotion::default().stream_fade(), std::time::Duration::ZERO);
    assert!((crate::motion::stream_curve().sample(0.) - crate::motion::LIFT).abs() < 1e-4);
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut n = 0;
    for f in std::fs::read_dir(&dir).expect("src").flatten() {
        let path = f.path();
        if path.extension().is_none_or(|e| e != "rs") || path.ends_with("ui_tests.rs") {
            continue;
        }
        let src = std::fs::read_to_string(&path).expect("a source file");
        n += 1;
        assert!(!src.contains(concat!(".mot", "ion(")), "{}", path.display());
        if !path.ends_with("motion.rs") {
            assert!(!src.contains(concat!("with_stream", "_fade")), "{}", path.display());
        }
        let policies = src.matches(concat!("set_mot", "ion(")).count();
        let ours = src.matches(concat!("motion::text_mot", "ion()")).count();
        assert!(policies == 0 || (path.ends_with("turns.rs") && policies == 1 && ours == 1), "{}", path.display());
    }
    assert!(n > 20, "every source file read ({n})");
}

/// A tray row leaving fades in its slot, but takes no click: the click
/// lands on what is there after the exit (nothing opens).
#[gpui_kit::test]
fn exiting_takes_no_click(cx: &mut TestAppContext) {
    use gpui_kit::{Modifiers, MouseButton, MouseDownEvent, MouseUpEvent};
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    settle(cx, w, 8);
    view.update(cx, |v, cx| {
        let row = v.tray.rows.first().cloned().expect("scout's row");
        v.tray.rows.clear();
        // Left "later": its fade holds at full for the whole test.
        v.tray.ghosts.push((0, row, v.fx.clock() + std::time::Duration::from_secs(60)));
        cx.notify();
    });
    cx.run_until_parked();
    let mut vcx = VisualTestContext::from_window(w, cx);
    let b = vcx.debug_bounds("tray-ghost").expect("the leaving row is drawn");
    let position = b.center();
    vcx.simulate_event(MouseDownEvent {
        button: MouseButton::Left,
        position,
        modifiers: Modifiers::default(),
        click_count: 1,
        first_mouse: false,
    });
    vcx.simulate_event(MouseUpEvent { button: MouseButton::Left, position, modifiers: Modifiers::default(), click_count: 1 });
    vcx.run_until_parked();
    assert!(view.read_with(&vcx, |v, _| v.model.watch.focus().is_none() && v.menu.is_none()), "the click opened nothing");
}

/// The answered card collapses in its own row (P13): drawn as it last was
/// at full strength, clipped from its height on `sheet`, taking no click;
/// nothing above it moves, and the call's row says what happened. Then the
/// row goes. Off: at once (below).
#[gpui_kit::test]
fn the_answered_card_collapses_in_its_row(cx: &mut TestAppContext) {
    use std::time::Duration;
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    settle(cx, w, 8);
    let bounds = |cx: &mut TestAppContext, sel: &str| {
        let sel: &'static str = Box::leak(sel.to_string().into_boxed_str());
        VisualTestContext::from_window(w, cx).debug_bounds(sel)
    };
    // A new prompt arrives while watched (the one there at open is history:
    // it doesn't breathe): its card traces and its glyph breathes.
    resolve_prompt(cx, &view);
    view.update(cx, |v, cx| {
        let request = synaps_client::protocol::PromptRequest {
            id: 8,
            kind: serde_json::json!("confirm"),
            title: "Confirm tool call".into(),
            prompt: "Tool: bash\n\nAllow this tool call?".into(),
            raised_at: None,
            confirm_scope: None,
        };
        v.model.transcript.apply(&SessionEvent::Prompt { request });
        cx.notify();
    });
    cx.run_until_parked();
    settle(cx, w, 8);
    let live = bounds(cx, "card").expect("the new card is up, in the log");
    let (call, row) = view.read_with(cx, |v, _| (v.waiting_on.expect("the call it gates"), v.current_card().unwrap().row));
    let call_row = bounds(cx, &format!("tool-row {call}")).expect("its call's row");
    assert!(live.top() >= call_row.bottom(), "the card's row is under its call's");
    let in_card = |cx: &mut TestAppContext, b: gpui_kit::Bounds<gpui_kit::Pixels>| {
        view.read_with(cx, |v, _| {
            v.fx.sites().iter().filter(|s| matches!(s.beat, crate::motion::Beat::Breath(_)) && b.contains(&s.origin)).count()
        })
    };
    // Cards D (cards4/proposal.md §8 #8, an intended change): the card asks
    // once, its dot pings; nothing on it breathes.
    assert_eq!(in_card(cx, live), 0, "the live card holds still on the beat layer");
    // The live card takes a press (the control for the collapsing one below).
    click_at(cx, w, live.center());
    assert_eq!(view.read_with(cx, |v, _| v.card_presses), 1, "the live card took the press");
    resolve_prompt(cx, &view);
    // Held at the collapse's start for the test.
    view.update(cx, |v, cx| {
        v.card_exit.as_mut().expect("answered on screen: it collapses").at = v.fx.clock() + Duration::from_secs(60);
        cx.notify();
    });
    cx.run_until_parked();
    assert!(bounds(cx, "card").is_none());
    let ghost = bounds(cx, "card-exit").expect("the answered card, collapsing");
    assert_eq!(ghost, live, "in its row: nothing above it moved");
    assert_eq!(bounds(cx, &format!("tool-row {call}")), Some(call_row), "its call's row holds still");
    assert_eq!(in_card(cx, ghost), 0, "collapsing, its glyph holds still: no breath through the clip");
    click_at(cx, w, ghost.center());
    assert_eq!(view.read_with(cx, |v, _| v.card_presses), 1, "the collapsing card took no press (the one is the live card's)");
    assert!(bounds(cx, &format!("receipt {row}")).is_some(), "the call's row says it was answered elsewhere");
    let slot = bounds(cx, &format!("grow-clip-{row}")).expect("its clip, whole");
    // Halfway: clipped shorter. Following, the list's end holds (F1); the
    // card folds up into its row, which holds still (the finish gate of
    // #23, item 7): the room it gave up waits at the list's end.
    view.update(cx, |v, cx| {
        v.card_exit.as_mut().unwrap().at = v.fx.clock() - Duration::from_millis(120);
        cx.notify();
    });
    cx.run_until_parked();
    let clip = bounds(cx, &format!("grow-clip-{row}")).expect("its clip");
    assert!(clip.size.height < live.size.height && clip.size.height > gpui_kit::px(1.), "{clip:?}");
    let r = cx.update(|cx| crate::theme::metrics(cx).r);
    assert!(view.read_with(cx, |v, _| v.list.is_following_tail()));
    let end = live.bottom() + gpui_kit::px((12. * r).round());
    assert!((clip.top() - slot.top()).abs() < gpui_kit::px(0.5), "the answered row holds: {clip:?} vs {slot:?}");
    let slack = bounds(cx, "fold-slack").expect("the room it gave up, at the list's end");
    assert!((slack.bottom() - end).abs() < gpui_kit::px(0.5), "the end holds: {slack:?} vs {end:?}");
    // Landed: the row goes.
    view.update(cx, |v, cx| {
        v.card_exit.as_mut().unwrap().at = v.fx.clock() - Duration::from_secs(5);
        cx.notify();
    });
    cx.run_until_parked();
    settle(cx, w, 8);
    assert!(view.read_with(cx, |v, _| v.card_exit.is_none()));
    assert!(bounds(cx, "card-exit").is_none() && bounds(cx, "card").is_none());
    assert!(view.read_with(cx, |v, _| v.disp.iter().all(|d| d.kind != crate::turns::Kind::Prompt)), "its row is gone");
    assert!(bounds(cx, &format!("receipt {row}")).is_some(), "the receipt stays");
    // Landed, the column still holds: the room waits at the end (until
    // rows that come use it), never a slide after the fold.
    assert_eq!(bounds(cx, &format!("tool-row {call}")), Some(call_row), "its call's row holds after the fold");
}

/// Cards D (cards4/proposal.md §8 #9, build step 10): Allow by its key
/// presses the button (drawn down, the accent's fill) through the fold's
/// first 90 ms; the card folds in its row on `glide`, and its call's row
/// takes the receipt, `Allowed` lit in the accent, then quiet.
#[gpui_kit::test]
fn allow_folds_the_card_into_its_row(cx: &mut TestAppContext) {
    use crate::authorize::Answer;
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    settle(cx, w, 8);
    let row = view.read_with(cx, |v, _| v.current_card().expect("the demo's card").row);
    let at = |cx: &mut TestAppContext, sel: String| VisualTestContext::from_window(w, cx).debug_bounds(String::leak(sel));
    assert!(at(cx, "allow-fill".into()).is_some(), "Allow is the accent's fill");
    card_keys(cx, w, &view);
    tap(cx, w, if cfg!(target_os = "macos") { "cmd-enter" } else { "ctrl-enter" });
    assert_eq!(view.read_with(cx, |v, _| v.sent.as_ref().map(|s| s.answer)), Some(Answer::Allowed));
    settle(cx, w, 2);
    let live = at(cx, "card".into()).expect("the live card, its answer sent");
    resolve_prompt(cx, &view);
    // The fold's first frame: in its row, Allow drawn pressed.
    assert_eq!(view.read_with(cx, |v, _| v.card_exit.as_ref().map(|e| e.answer)), Some(Answer::Allowed), "answered on screen: it folds");
    let ghost = at(cx, "card-exit".into()).expect("the answered card, folding");
    assert_eq!(ghost, live, "in its row");
    let pressed = at(cx, "allow-fill".into()).expect("Allow, pressed");
    assert!(ghost.contains(&pressed.center()), "the pressed Allow is the folding card's");
    assert!(at(cx, format!("receipt-word {row} allowed")).is_some(), "the receipt lands as it folds");
    wait(cx, w, &view, 1000);
    assert!(at(cx, "card-exit".into()).is_none() && at(cx, "card".into()).is_none(), "folded away");
    assert!(view.read_with(cx, |v, _| v.card_exit.is_none()));
    assert!(at(cx, format!("receipt-word {row} allowed")).is_some(), "its call's row keeps the receipt");
}

/// Shady #23 r2 M3: the room an answered card left at the list's end holds
/// the answered row still while its call's output grows into it (the
/// list's top held: the room, read from the last layout, lags a frame, and
/// that frame's surplus is under the content), and a scroll up within the
/// room sends it on `sheet`: no frame moves the row by more than about
/// 40 px a 60 Hz frame (17 px a 144 Hz test frame).
#[gpui_kit::test]
fn the_held_room_never_bobs_or_drops(cx: &mut TestAppContext) {
    use synaps_client::protocol::{LlmEvent, StreamEvent};
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    view.update(cx, |v, _| {
        v.client = None;
        v.model.transcript.set_busy(true);
    });
    settle(cx, w, 8);
    let (tool, id) = view.read_with(cx, |v, _| {
        let d = v.disp.iter().find(|d| d.kind == crate::turns::Kind::Prompt).expect("the demo's card");
        let i = &v.model.transcript.items()[d.item];
        (i.tool.as_ref().expect("the card's call").tool_id.clone(), i.id)
    });
    resolve_prompt(cx, &view);
    wait(cx, w, &view, 1000);
    assert!(view.read_with(cx, |v, _| v.fold_slack.get().is_some()), "the room it left, held");
    let top = |cx: &mut TestAppContext| {
        VisualTestContext::from_window(w, cx)
            .debug_bounds(String::leak(format!("tool-row {id}")))
            .map(|b| f32::from(b.top()))
            .expect("the answered row")
    };
    let at = top(cx);
    let mut held = 0;
    for k in 0..6 {
        view.update(cx, |v, cx| {
            v.model.transcript.apply(&SessionEvent::Stream {
                event: StreamEvent::Llm(LlmEvent::ToolResultDelta { tool_id: tool.clone(), delta: format!("line {k}\n") }),
            });
            v.sync_list();
            cx.notify();
        });
        cx.run_until_parked();
        for _ in 0..30 {
            fluid_frame(cx, w, &view);
            if view.read_with(cx, |v, _| v.fold_slack.get().is_some()) {
                held += 1;
                let y = top(cx);
                assert!((y - at).abs() <= 0.5, "line {k}: the answered row at {y}, not {at}");
            }
        }
    }
    assert!(held > 30, "the output grew into the room ({held} frames)");
    // A scroll up within what is left (if any): on `sheet`, never a drop.
    let left = view.read_with(cx, |v, _| v.tail_slack());
    if left > 41. {
        view.update(cx, |v, cx| {
            v.list.scroll_by(px(-40.));
            cx.notify();
        });
        let mut y0 = top(cx);
        for _ in 0..200 {
            fluid_frame(cx, w, &view);
            let y = top(cx);
            assert!((y - y0).abs() <= 17., "scrolled up: the row moved {} px in one frame", y - y0);
            y0 = y;
        }
    }
}

/// Shady #23 r2b L1: at 125 % GPUI snaps rows to device pixels (0.8 px), so
/// the list's sums go fractional again. The probe: following a stream, the
/// end is the bottom within a device pixel every frame; while an answered
/// call's output grows into the room its card left, the answered row moves
/// at most a device pixel in a frame: the offset's 0.8 px steps are not
/// read as a scroll leaving the end, so the room never drops.
#[gpui_kit::test]
fn the_end_and_the_held_room_at_125_percent(cx: &mut TestAppContext) {
    use synaps_client::protocol::{LlmEvent, StreamEvent};
    let (w, view, _events) = open(cx);
    VisualTestContext::from_window(w, cx).simulate_scale_factor_change(1.25);
    set_motion(cx, &view, crate::motion::Level::Full);
    view.update(cx, |v, _| v.client = None);
    start_stream(cx, &view);
    settle(cx, w, 8);
    view.update(cx, |v, _| v.list.scroll_to_end());
    fluid_frame(cx, w, &view);
    for k in 0..60 {
        stream_step(cx, &view, k);
        fluid_frame(cx, w, &view);
        let (gap, pad) = view.read_with(cx, |v, cx| {
            let n = v.list.item_count();
            let pad = (28. * crate::theme::metrics(cx).r).round();
            (v.list.bounds_for_item(n - 1).map(|b| f32::from(v.list.viewport_bounds().bottom() - b.bottom())), pad)
        });
        let gap = gap.expect("the last row");
        assert!((gap - pad).abs() <= 0.85, "125 %, frame {k}: the end {gap} px over the bottom, not {pad}");
    }
    let (w, view, _events) = open(cx);
    VisualTestContext::from_window(w, cx).simulate_scale_factor_change(1.25);
    set_motion(cx, &view, crate::motion::Level::Full);
    view.update(cx, |v, _| {
        v.client = None;
        v.model.transcript.set_busy(true);
    });
    settle(cx, w, 8);
    let (tool, id) = view.read_with(cx, |v, _| {
        let d = v.disp.iter().find(|d| d.kind == crate::turns::Kind::Prompt).expect("the demo's card");
        let i = &v.model.transcript.items()[d.item];
        (i.tool.as_ref().expect("the card's call").tool_id.clone(), i.id)
    });
    resolve_prompt(cx, &view);
    wait(cx, w, &view, 1000);
    assert!(view.read_with(cx, |v, _| v.fold_slack.get().is_some()), "125 %: the room it left, held");
    let top = |cx: &mut TestAppContext| {
        VisualTestContext::from_window(w, cx)
            .debug_bounds(String::leak(format!("tool-row {id}")))
            .map(|b| f32::from(b.top()))
            .expect("the answered row")
    };
    let (at, mut y0) = (top(cx), top(cx));
    for k in 0..6 {
        view.update(cx, |v, cx| {
            v.model.transcript.apply(&SessionEvent::Stream {
                event: StreamEvent::Llm(LlmEvent::ToolResultDelta { tool_id: tool.clone(), delta: format!("line {k}\n") }),
            });
            v.sync_list();
            cx.notify();
        });
        cx.run_until_parked();
        for _ in 0..30 {
            fluid_frame(cx, w, &view);
            if view.read_with(cx, |v, _| v.fold_slack.get().is_some()) {
                let y = top(cx);
                assert!((y - y0).abs() <= 0.85, "125 %, line {k}: the answered row moved {y0} -> {y} in one frame (from {at})");
                y0 = y;
            }
        }
    }
}

/// The card's keys: the window's focus (a click on the card), a frame for
/// them to go live, and their arm's wait (real time: [`crate::app::Arm`]).
fn card_keys(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    cx.update_window(w, |_, window, cx| {
        window.activate_window();
        view.update(cx, |v, cx| window.focus(&v.focus, cx))
    })
    .expect("the window");
    cx.run_until_parked();
    settle(cx, w, 8);
    assert_eq!(view.read_with(cx, |v, _| v.card_keys.get()), Some(false), "the card has the keys");
    std::thread::sleep(crate::app::ARM + std::time::Duration::from_millis(60));
}

/// Q2 (polish.md §2.6): Allow all is the input owner's. Not the owner (the
/// session's input shared, another client owning it): no box, Space does
/// nothing, ALLOW sends `y`. The owner: the box; Space checks it only with
/// the box focused (clicked), a new press, the keys armed (Shady M2), and
/// every toggle re-arms the keys, so ALLOW right after it does nothing;
/// once armed again ALLOW sends `"always"`. The header says what asked; a
/// guard's well holds its command and the folder line its folder.
#[gpui_kit::test]
fn allow_all_is_the_input_owners(cx: &mut TestAppContext) {
    use crate::authorize::Answer;
    let (w, view, _events) = open(cx);
    let has = |cx: &mut TestAppContext, sel: &'static str| VisualTestContext::from_window(w, cx).debug_bounds(sel).is_some();
    assert!(has(cx, "card") && has(cx, "card-asked") && has(cx, "card-folder") && has(cx, "prompt-well"));
    let set_owner = |cx: &mut TestAppContext, owner: u64| {
        view.update(cx, |v, cx| {
            v.model.active.as_mut().unwrap().owner = Some(owner);
            cx.notify();
        });
        cx.run_until_parked();
    };
    let checked = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.current_card().is_some_and(|c| c.allow_all));
    set_owner(cx, 99);
    card_keys(cx, w, &view);
    assert!(view.read_with(cx, |v, _| v.drives()), "shared input: this window still answers");
    assert!(!has(cx, "card-allow-all"), "not the owner: not offered");
    tap(cx, w, "space");
    assert!(!checked(cx), "Space checks nothing that isn't offered");
    set_owner(cx, 2);
    assert!(has(cx, "card-allow-all") && !has(cx, "card-allow-all-on"));
    tap(cx, w, "space");
    assert!(!checked(cx), "Space with the card's keys but not the box's focus: nothing");
    // Tab from the card's keys: the box has the focus; Space toggles it and
    // re-arms the keys. Shift+Tab: back to the card, where Space does not.
    let box_focus = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.current_card().is_some_and(|c| c.box_focus));
    tap(cx, w, "tab");
    settle(cx, w, 4);
    assert!(box_focus(cx), "Tab focuses the box");
    assert_eq!(view.read_with(cx, |v, _| v.card_keys.get()), Some(false), "the card keeps its keys");
    tap(cx, w, "space");
    assert!(checked(cx), "Tab, then Space checks it");
    tap(cx, w, if cfg!(target_os = "macos") { "cmd-enter" } else { "ctrl-enter" });
    assert!(view.read_with(cx, |v, _| v.sent.is_none()), "the toggle re-armed the keys");
    tap(cx, w, "shift-tab");
    settle(cx, w, 4);
    assert!(!box_focus(cx), "Shift+Tab: back to the card");
    std::thread::sleep(crate::app::ARM + std::time::Duration::from_millis(60));
    tap(cx, w, "space");
    assert!(checked(cx), "Space off the box: unchanged");
    tap(cx, w, "tab");
    settle(cx, w, 4);
    tap(cx, w, "space");
    assert!(!checked(cx), "Tab, Space: unchecked");
    tap(cx, w, "tab");
    settle(cx, w, 4);
    assert!(!box_focus(cx), "Tab on the box: back to the card");
    std::thread::sleep(crate::app::ARM + std::time::Duration::from_millis(60));
    // A frame with the box armed again (its click listener with it).
    view.update(cx, |_, cx| cx.notify());
    settle(cx, w, 4);
    let at = VisualTestContext::from_window(w, cx).debug_bounds("card-allow-all").expect("the box").center();
    click_at(cx, w, at);
    settle(cx, w, 8);
    assert!(checked(cx) && has(cx, "card-allow-all-on"), "a click checks it");
    assert!(view.read_with(cx, |v, _| v.current_card().is_some_and(|c| c.box_focus)), "and focuses it");
    let yes = if cfg!(target_os = "macos") { "cmd-enter" } else { "ctrl-enter" };
    tap(cx, w, yes);
    assert!(view.read_with(cx, |v, _| v.sent.is_none()), "the toggle re-armed the keys: ALLOW waits");
    std::thread::sleep(crate::app::ARM + std::time::Duration::from_millis(60));
    tap(cx, w, "space");
    assert!(!checked(cx), "Space on the focused box unchecks it");
    std::thread::sleep(crate::app::ARM + std::time::Duration::from_millis(60));
    tap(cx, w, "space");
    assert!(checked(cx), "and checks it");
    // A held Space toggles nothing more (its repeats are not presses).
    down(cx, w, "space", true);
    up(cx, w, "space");
    assert!(checked(cx), "held: unchanged");
    std::thread::sleep(crate::app::ARM + std::time::Duration::from_millis(60));
    tap(cx, w, yes);
    assert_eq!(view.read_with(cx, |v, _| v.sent.as_ref().map(|s| s.answer)), Some(Answer::AllowedAll), "ALLOW sent \"always\"");
    // How long you took counts from the card's first showing, not from
    // the last toggle's re-arm (live: `allowed all · 1s` after 20 s).
    let (took, shown) = view.read_with(cx, |v, _| (v.sent.as_ref().unwrap().took, v.current_card().unwrap().shown.elapsed()));
    assert!(took + std::time::Duration::from_millis(250) >= shown && took >= 3 * crate::app::ARM, "took {took:?}, shown {shown:?} ago");
    // Answered: Space no longer toggles it (the card can't be answered).
    tap(cx, w, "space");
    assert!(checked(cx), "answer sent: unchanged");
    // Resolved: the receipt in the call's row; the card's state goes.
    resolve_prompt(cx, &view);
    wait(cx, w, &view, 1000);
    view.read_with(cx, |v, _| {
        assert!(v.card.is_none() && v.card_exit.is_none() && v.sent.is_none(), "authorize_entity_dropped_on_answer");
        let r = v.receipts.values().flatten().last().expect("a receipt");
        let call =
            v.model.transcript.items().iter().find(|i| Some(i.id) == v.waiting_on).and_then(|i| i.tool.as_ref()).map(|t| t.tool_id.clone());
        assert_eq!((r.answer, r.own, Some(r.call.to_string())), (Answer::AllowedAll, false, call));
    });
}

/// Q2: the card scrolled away, the NEEDS YOU stub stands in over the
/// chips; Ctrl+J (from the composer too) brings the card back into view,
/// the keys with it, and the stub goes.
#[gpui_kit::test]
fn ctrl_j_goes_to_the_card(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    let has = |cx: &mut TestAppContext, sel: &'static str| VisualTestContext::from_window(w, cx).debug_bounds(sel).is_some();
    assert!(view.read_with(cx, |v, _| v.card_on_screen()) && !has(cx, "needs-stub"), "on screen: no stub");
    view.update(cx, |v, cx| {
        v.list.scroll_to(gpui_kit::ListOffset { item_ix: 0, offset_in_item: gpui_kit::px(0.) });
        cx.notify();
    });
    settle(cx, w, 8);
    settle(cx, w, 8);
    assert!(view.read_with(cx, |v, _| !v.card_on_screen()), "scrolled away");
    assert!(has(cx, "needs-stub") && has(cx, "needs-stub-key"), "the stub says ctrl j");
    focus_composer(cx, w, &view);
    // The card's top, frame by frame: one way (the gate's S2, PR #21: the
    // stub leaves as the jump starts, which aims at the view without it).
    let card_top_of = |v: &AppView| v.card_ix().and_then(|ix| v.list.bounds_for_item(ix)).map(|b| f32::from(b.top()));
    let card_top = |cx: &mut TestAppContext| view.read_with(cx, |v, _| card_top_of(v));
    let one_way = |tops: &[f32]| {
        let d: Vec<f32> = tops.windows(2).map(|p| p[1] - p[0]).collect();
        d.iter().all(|d| *d <= 0.01) || d.iter().all(|d| *d >= -0.01)
    };
    tap(cx, w, "ctrl-j");
    // F1's glide, frame by frame.
    let mut tops = Vec::new();
    for _ in 0..120 {
        fluid_frame(cx, w, &view);
        tops.extend(card_top(cx));
    }
    settle(cx, w, 8);
    assert!(view.read_with(cx, |v, _| v.card_on_screen()), "back in view");
    assert!(!has(cx, "needs-stub"), "the stub goes");
    assert!(!view.read_with(cx, |v, cx| v.composer.read(cx).text().to_string().contains('\n')), "Ctrl+J typed nothing");
    assert!(tops.len() > 10 && one_way(&tops), "one way: {tops:?}");
    // Just under the view's end (the stub over it): it goes no further up
    // than the view without the stub needs, and not back down.
    view.update(cx, |v, cx| {
        v.list.scroll_to_end();
        cx.notify();
    });
    settle(cx, w, 8);
    let (t0, vb) = view.read_with(cx, |v, _| (card_top_of(v), v.list_view().1));
    view.update(cx, |v, cx| {
        v.list.scroll_by(px(-(vb + 10. - t0.expect("the card drawn"))));
        cx.notify();
    });
    cx.run_until_parked();
    settle(cx, w, 8);
    fly(cx, w, &view, 400, |_| {});
    settle(cx, w, 8);
    assert!(has(cx, "needs-stub"), "under the view: the stub");
    tap(cx, w, "ctrl-j");
    let mut tops: Vec<f32> = card_top(cx).into_iter().collect();
    for _ in 0..120 {
        fluid_frame(cx, w, &view);
        tops.extend(card_top(cx));
    }
    settle(cx, w, 8);
    assert!(view.read_with(cx, |v, _| v.card_on_screen()) && !has(cx, "needs-stub"), "in view, no stub");
    assert!(one_way(&tops), "one way: {tops:?}");
}

/// Q2: an answer from this window is its receipt (`denied` here); one this
/// window did not send is `answered elsewhere`, with no wait.
#[gpui_kit::test]
fn a_receipt_says_who_answered(cx: &mut TestAppContext) {
    use crate::authorize::Answer;
    let (w, view, _events) = open(cx);
    card_keys(cx, w, &view);
    tap(cx, w, "escape");
    assert_eq!(view.read_with(cx, |v, _| v.sent.as_ref().map(|s| s.answer)), Some(Answer::Denied), "Esc denies");
    resolve_prompt(cx, &view);
    wait(cx, w, &view, 1000);
    let call = view.read_with(cx, |v, _| {
        let r = v.receipts.values().flatten().last().expect("a receipt");
        assert_eq!(r.answer, Answer::Denied);
        assert!(r.took.is_some());
        let items = v.model.transcript.items();
        items[crate::authorize::call_ix(items, &r.call).expect("its call")].id
    });
    let words = view.read_with(cx, |v, _| v.receipts.values().flatten().last().unwrap().words());
    assert_eq!(words.0, "denied");
    assert!(VisualTestContext::from_window(w, cx).debug_bounds(String::leak(format!("tool-row {call}"))).is_some());
    // Another prompt, resolved without this window answering.
    view.update(cx, |v, cx| {
        let request = synaps_client::protocol::PromptRequest {
            id: 9,
            kind: serde_json::json!("confirm"),
            title: "Confirm tool call".into(),
            prompt: "Tool: bash\n\nAllow this tool call?".into(),
            raised_at: None,
            confirm_scope: None,
        };
        v.model.transcript.apply(&SessionEvent::Prompt { request });
        cx.notify();
    });
    cx.run_until_parked();
    settle(cx, w, 8);
    resolve_prompt(cx, &view);
    wait(cx, w, &view, 1000);
    view.read_with(cx, |v, _| {
        let r = v.receipts.values().flatten().last().unwrap();
        assert_eq!((r.answer, r.took), (Answer::Resolved, None), "this window owns the input: nothing says who");
        assert!(r.words().0 == "resolved");
    });
    // Resolved while another client owns the input: answered elsewhere.
    raise(cx, &view, 10);
    settle(cx, w, 8);
    view.update(cx, |v, _| v.model.active.as_mut().unwrap().owner = Some(99));
    resolve_prompt(cx, &view);
    wait(cx, w, &view, 1000);
    assert_eq!(view.read_with(cx, |v, _| v.receipts.values().flatten().last().unwrap().answer), Answer::Elsewhere);
    // Shady M4: HALT from here, then the daemon resolves the prompt (with
    // no answer: the tool is blocked): cancelled, never answered elsewhere.
    view.update(cx, |v, _| v.model.active.as_mut().unwrap().owner = Some(2));
    raise(cx, &view, 11);
    settle(cx, w, 8);
    view.update(cx, |v, _| v.halt());
    assert!(view.read_with(cx, |v, _| v.halted.is_some()), "this window halted");
    resolve_prompt(cx, &view);
    wait(cx, w, &view, 1000);
    assert_eq!(view.read_with(cx, |v, _| v.receipts.values().flatten().last().unwrap().answer), Answer::Cancelled);
    // The turn already over (another client's HALT, a revoke): cancelled too.
    view.update(cx, |v, _| v.halted = None);
    raise(cx, &view, 12);
    settle(cx, w, 8);
    view.update(cx, |v, _| v.model.transcript.set_busy(false));
    resolve_prompt(cx, &view);
    wait(cx, w, &view, 1000);
    assert_eq!(view.read_with(cx, |v, _| v.receipts.values().flatten().last().unwrap().answer), Answer::Cancelled);
    // Shady N3: answered elsewhere while the turn ran, the turn over before
    // this window drew again: what it was when it resolved, not cancelled.
    view.update(cx, |v, _| {
        v.model.transcript.set_busy(true);
        v.model.active.as_mut().unwrap().owner = Some(99);
    });
    raise(cx, &view, 13);
    settle(cx, w, 8);
    view.update(cx, |v, _| {
        v.model.transcript.apply(&SessionEvent::PromptResolved { prompt_id: 13 });
        v.model.transcript.apply(&SessionEvent::Idle);
    });
    wait(cx, w, &view, 1000);
    assert_eq!(view.read_with(cx, |v, _| v.receipts.values().flatten().last().unwrap().answer), Answer::Elsewhere);
}

/// Shady N2: a prompt with no call in its turn (a secret at the turn's
/// start) gets no receipt from an earlier turn's call, and still collapses.
#[gpui_kit::test]
fn a_prompt_with_no_call_in_its_turn_still_collapses(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    resolve_prompt(cx, &view);
    settle(cx, w, 8);
    view.update(cx, |v, cx| {
        v.model.transcript.apply(&SessionEvent::TurnStarted { turn_baseline: 0, trigger: String::new(), user_text: Some("hi".into()) });
        let request = synaps_client::protocol::PromptRequest {
            id: 21,
            kind: serde_json::json!("secret"),
            title: "Secret requested".into(),
            prompt: "Password for sudo".into(),
            raised_at: None,
            confirm_scope: None,
        };
        v.model.transcript.apply(&SessionEvent::Prompt { request });
        cx.notify();
    });
    cx.run_until_parked();
    settle(cx, w, 8);
    let (row, before) =
        view.read_with(cx, |v, _| (v.current_card().expect("the secret's card").row, v.receipts.values().flatten().count()));
    resolve_prompt(cx, &view);
    view.read_with(cx, |v, _| {
        assert!(v.card_exit.as_ref().is_some_and(|e| e.card.row == row && e.to == 0.), "it collapses, to nothing");
        assert_eq!(v.receipts.values().flatten().count(), before, "no receipt on an earlier turn's call");
    });
}

/// The finish gate of #18 and Shady N1: a gated row's meta reads in order,
/// each part apart: `3s running · allowed 19:14` while it runs (its clock
/// from the answer, the wait left to the receipt), then `failed · allowed
/// 19:14 · 15s`.
#[gpui_kit::test]
fn a_gated_rows_meta_reads_apart(cx: &mut TestAppContext) {
    use crate::authorize::Answer;
    let (w, view, _events) = open(cx);
    // Shady N4: the card built anew for the same prompt (a tab's round
    // trip) keeps its first showing: how long you took counts from there.
    let first = view.read_with(cx, |v, _| v.current_card().unwrap().shown);
    view.update(cx, |v, cx| {
        v.card = None;
        cx.notify();
    });
    settle(cx, w, 8);
    assert_eq!(view.read_with(cx, |v, _| v.current_card().map(|c| c.shown)), Some(first), "shown when first shown");
    card_keys(cx, w, &view);
    tap(cx, w, if cfg!(target_os = "macos") { "cmd-enter" } else { "ctrl-enter" });
    assert_eq!(view.read_with(cx, |v, _| v.sent.as_ref().map(|s| s.answer)), Some(Answer::Allowed));
    resolve_prompt(cx, &view);
    wait(cx, w, &view, 1000);
    let (row, call, tool) = view.read_with(cx, |v, _| {
        let r = v.receipts.values().flatten().last().expect("a receipt");
        assert!(!r.own && r.took.is_some());
        let items = v.model.transcript.items();
        let it = &items[crate::authorize::call_ix(items, &r.call).expect("its call")];
        (r.row, it.id, r.call.to_string())
    });
    let at = |cx: &mut TestAppContext, sel: String| VisualTestContext::from_window(w, cx).debug_bounds(String::leak(sel));
    let parts = |cx: &mut TestAppContext| (at(cx, format!("receipt-sep {row}")), at(cx, format!("receipt {row}")));
    let (sep, words) = parts(cx);
    let (sep, words) = (sep.expect("a · before the receipt"), words.expect("the receipt"));
    assert!(sep.right() <= words.left() && at(cx, format!("tool-row {call}")).is_some_and(|b| b.contains(&sep.center())));
    // It fails: `failed · allowed 14:21 · 0s`, never `failed allowed`.
    view.update(cx, |v, cx| {
        let ev = serde_json::json!({"ev": "stream", "event": {"kind": "llm", "llm": "tool_result", "tool_id": tool, "result": "Unknown tool: shell_strat"}});
        v.model.transcript.apply(&synaps_client::protocol::decode_event(&ev).expect("a result"));
        cx.notify();
    });
    settle(cx, w, 8);
    let st = view.read_with(cx, |v, _| {
        v.model.transcript.items().iter().find(|i| i.id == call).and_then(|i| i.tool.as_ref()).and_then(|t| t.status)
    });
    assert_eq!(st, Some(synaps_client::transcript::ToolStatus::Failed));
    let (sep, words) = parts(cx);
    assert!(sep.zip(words).is_some_and(|(s, r)| s.right() <= r.left()), "failed · allowed …");
}

/// The card up, offering Allow all (this window owns the input), its keys
/// live and armed.
fn allow_all_up(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    view.update(cx, |v, cx| {
        v.model.active.as_mut().unwrap().owner = Some(2);
        cx.notify();
    });
    cx.run_until_parked();
    card_keys(cx, w, view);
    assert!(VisualTestContext::from_window(w, cx).debug_bounds("card-allow-all").is_some(), "Allow all offered");
}

fn box_focused(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) -> bool {
    cx.update_window(w, |_, window, cx| view.read(cx).allow_all_focus.is_focused(window)).expect("the window")
}

/// Shady M1 of round 4: the Allow all box is no tab stop. Tab and Shift+Tab
/// from the composer never reach it, so a space typed there never checks it.
#[gpui_kit::test]
fn the_composers_tab_never_reaches_allow_all(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    allow_all_up(cx, w, &view);
    cx.update_window(w, |_, window, cx| view.update(cx, |v, cx| v.composer.update(cx, |s, cx| s.focus(window, cx)))).expect("the window");
    settle(cx, w, 4);
    assert!(composer_focused(cx, w, &view), "typing a steer");
    for key in ["shift-tab", "tab", "tab", "shift-tab", "shift-tab"] {
        tap(cx, w, key);
        settle(cx, w, 4);
        assert!(!box_focused(cx, w, &view), "{key} from the composer: never the box");
        assert_ne!(view.read_with(cx, |v, _| v.card_keys.get()), Some(false), "{key}: the card's keys are not live");
    }
    std::thread::sleep(crate::app::ARM + std::time::Duration::from_millis(60));
    tap(cx, w, "space");
    settle(cx, w, 4);
    assert!(view.read_with(cx, |v, _| v.current_card().is_some_and(|c| !c.allow_all)), "a typed space: still unchecked");
}

/// Shady L1 of round 4: with a card up, Tab in an open menu (a worker's, a
/// session's, the workers menu) is the menu's, never a way to the box.
#[gpui_kit::test]
fn tab_in_a_menu_is_the_menus(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    allow_all_up(cx, w, &view);
    // The open menu's selection, and how many of its items can act.
    let sel = |cx: &mut TestAppContext| {
        view.read_with(cx, |v, _| {
            fn of<A>(m: &crate::menu::Menu<A>) -> (Option<usize>, usize) {
                (m.sel, m.items.iter().filter(|i| i.enabled()).count())
            }
            v.menu.as_ref().map(|(m, _)| of(m)).or(v.smenu.as_ref().map(|(m, _)| of(m))).or(v.wmenu.as_ref().map(of)).expect("a menu")
        })
    };
    let mut stepped = 0;
    let mut after_tab = |cx: &mut TestAppContext, what: &str| {
        let root = cx.update_window(w, |_, window, cx| view.read(cx).focus.is_focused(window)).expect("the window");
        assert!(root, "{what}: the window has the focus");
        assert_eq!(view.read_with(cx, |v, _| v.card_keys.get()), None, "{what}: the card's keys wait");
        let (was, can) = sel(cx);
        tap(cx, w, "tab");
        settle(cx, w, 4);
        assert!(!box_focused(cx, w, &view), "{what}: Tab is the menu's");
        let (now, _) = sel(cx);
        if can > 1 {
            assert_ne!(now, was, "{what}: Tab stepped it");
            stepped += 1;
        }
    };
    let close = |cx: &mut TestAppContext| {
        cx.update_window(w, |_, window, cx| {
            view.update(cx, |v, cx| {
                v.close_menu();
                window.focus(&v.focus, cx);
                cx.notify();
            })
        })
        .expect("the window");
        settle(cx, w, 4);
    };
    // A worker's menu, from the keyboard over scout's tray row.
    down(cx, w, "shift-f10", false);
    up(cx, w, "shift-f10");
    settle(cx, w, 4);
    assert!(view.read_with(cx, |v, _| v.menu.is_some()), "Shift+F10: scout's menu");
    after_tab(cx, "a worker's menu");
    close(cx);
    // A session's.
    let key = view.read_with(cx, |v, _| v.rail.rail.rows.iter().find(|r| r.live.is_some()).map(|r| r.key)).expect("a live row");
    cx.update_window(w, |_, window, cx| {
        view.update(cx, |v, cx| v.open_session_menu(crate::app::SessionAt::Row(key), Default::default(), window, cx))
    })
    .expect("the window");
    settle(cx, w, 4);
    assert!(view.read_with(cx, |v, _| v.smenu.is_some()), "the session menu");
    after_tab(cx, "a session's menu");
    close(cx);
    // The workers menu (`+N more` of a fan-out wider than its chips' lines).
    let rows: Vec<serde_json::Value> = (1..=12u64)
        .map(|n| {
            serde_json::json!({"subagent_id": n, "agent_name": format!("worker-{n}"), "status": "running", "cancel_requested": false,
                "elapsed_secs": 10.0 + n as f64, "finished_elapsed": null, "step": format!("reading src/part_{n}.rs"), "tools": n,
                "handle_id": format!("sa_{n}")})
        })
        .collect();
    let rows = synaps_client::protocol::decode_event(&serde_json::json!({"ev": "subagent_rows", "rows": rows})).expect("a rows event");
    view.update(cx, |v, cx| {
        v.model.stats.subagents.on_session(&rows);
        cx.notify();
    });
    settle(cx, w, 8);
    cx.update_window(w, |_, window, cx| view.update(cx, |v, cx| v.open_workers_menu(Default::default(), window, cx))).expect("the window");
    settle(cx, w, 4);
    assert!(view.read_with(cx, |v, _| v.wmenu.as_ref().is_some_and(|m| m.items.len() > 1)), "the workers menu");
    after_tab(cx, "the workers menu");
    assert!(stepped > 0, "a menu with items to step to");
}

/// Shady L3 of round 4: an early Space on the box starts the wait again,
/// like an early Ctrl+Enter or Esc ([`crate::app::Arm`]).
#[gpui_kit::test]
fn an_early_space_on_the_box_waits_again(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    allow_all_up(cx, w, &view);
    let checked = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.current_card().is_some_and(|c| c.allow_all));
    let arm = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.current_card().unwrap().arm);
    tap(cx, w, "tab");
    settle(cx, w, 4);
    assert!(box_focused(cx, w, &view));
    tap(cx, w, "space");
    assert!(checked(cx), "armed: Space checks it (and re-arms)");
    let toggled = arm(cx);
    tap(cx, w, "space");
    assert!(checked(cx), "too early: unchanged");
    assert_ne!(arm(cx), toggled, "and the keys wait from this Space");
    std::thread::sleep(crate::app::ARM + std::time::Duration::from_millis(60));
    tap(cx, w, "space");
    assert!(!checked(cx), "armed again: Space unchecks it");
}

/// Shady L4 of round 4: resolved while another client owns the input, its
/// call then cancelled (a HALT from there): the row reads `cancelled`, once,
/// never `cancelled · answered elsewhere`.
#[gpui_kit::test]
fn a_halt_from_elsewhere_reads_cancelled(cx: &mut TestAppContext) {
    use crate::authorize::Answer;
    let (w, view, _events) = open(cx);
    settle(cx, w, 8);
    view.update(cx, |v, _| v.model.active.as_mut().unwrap().owner = Some(99));
    resolve_prompt(cx, &view);
    wait(cx, w, &view, 1000);
    let (row, call, tool) = view.read_with(cx, |v, _| {
        let r = v.receipts.values().flatten().last().expect("a receipt");
        assert_eq!(r.answer, Answer::Elsewhere);
        let items = v.model.transcript.items();
        let it = &items[crate::authorize::call_ix(items, &r.call).expect("its call")];
        (r.row, it.id, r.call.to_string())
    });
    let has = |cx: &mut TestAppContext, sel: String| VisualTestContext::from_window(w, cx).debug_bounds(String::leak(sel)).is_some();
    assert!(has(cx, format!("receipt-word {row} answered elsewhere")));
    view.update(cx, |v, cx| {
        let ev = serde_json::json!({"ev": "stream", "event": {"kind": "llm", "llm": "tool_result", "tool_id": tool, "result": "Canceled by user"}});
        v.model.transcript.apply(&synaps_client::protocol::decode_event(&ev).expect("a result"));
        cx.notify();
    });
    settle(cx, w, 8);
    let st = view.read_with(cx, |v, _| {
        v.model.transcript.items().iter().find(|i| i.id == call).and_then(|i| i.tool.as_ref()).and_then(|t| t.status)
    });
    assert_eq!(st, Some(synaps_client::transcript::ToolStatus::Cancelled));
    assert!(has(cx, format!("receipt-word {row} cancelled")), "the receipt says cancelled");
    assert!(!has(cx, format!("receipt-word {row} answered elsewhere")), "not answered elsewhere");
    assert!(!has(cx, format!("tool-word {call}")), "and the row's status word is not said twice");
}

/// A guard's confirm for the demo's waiting call, id `id`, raised now.
fn raise(cx: &mut TestAppContext, view: &Entity<AppView>, id: u64) {
    view.update(cx, |v, cx| {
        let request = synaps_client::protocol::PromptRequest {
            id,
            kind: serde_json::json!("confirm"),
            title: "Confirm tool call".into(),
            prompt: "Tool: bash\n\nAllow this tool call?".into(),
            raised_at: None,
            confirm_scope: None,
        };
        v.model.transcript.apply(&SessionEvent::Prompt { request });
        cx.notify();
    });
    cx.run_until_parked();
}

/// Shady M1: three calls from one response, all unfinished. The engine runs
/// them in order, so the confirm is the first's: its card under the first,
/// unless the prompt names another (its scope's tool, a guard's command).
#[gpui_kit::test]
fn the_card_goes_under_the_call_it_gates(cx: &mut TestAppContext) {
    use synaps_client::protocol::{LlmEvent, StreamEvent};
    let (w, view, _events) = open(cx);
    resolve_prompt(cx, &view);
    wait(cx, w, &view, 1000);
    // The demo's call done, three new ones from one response.
    view.update(cx, |v, _| {
        let id =
            v.model.transcript.items().iter().find(|i| Some(i.id) == v.waiting_on).and_then(|i| i.tool.as_ref()).unwrap().tool_id.clone();
        v.model
            .transcript
            .apply(&SessionEvent::Stream { event: StreamEvent::Llm(LlmEvent::ToolResult { tool_id: id, result: "ok".into() }) });
    });
    running_tools(cx, &view, 3);
    let id_of =
        |v: &AppView, c: &str| v.model.transcript.items().iter().find(|i| i.tool.as_ref().is_some_and(|t| t.tool_id == c)).map(|i| i.id);
    view.read_with(cx, |v, _| {
        let items = v.model.transcript.items();
        assert_eq!(crate::turns::waiting_call(items, None), id_of(v, "ui-run-0"), "the first unfinished");
        let named = serde_json::from_value::<synaps_client::protocol::PromptRequest>(serde_json::json!({
            "id": 30, "kind": "confirm", "title": "Confirm tool call",
            "prompt": "Extension: guard\nTool: bash\n\nMessage from guard:\nbash wants to run a command in ~/w:\n$ cargo test -p crate1\n\nAllow this tool call?",
            "confirm_scope": {"extensions": ["guard"], "tool": "bash"}
        }))
        .unwrap();
        assert_eq!(crate::turns::waiting_call(items, Some(&named)), id_of(v, "ui-run-1"), "the one its command names");
        let other = serde_json::from_value::<synaps_client::protocol::PromptRequest>(serde_json::json!({
            "id": 31, "kind": "confirm", "title": "Confirm tool call", "prompt": "Tool: edit",
            "confirm_scope": {"extensions": ["lint"], "tool": "edit"}
        }))
        .unwrap();
        assert_eq!(crate::turns::waiting_call(items, Some(&other)), id_of(v, "ui-run-0"), "none named: the first");
    });
    raise(cx, &view, 20);
    settle(cx, w, 8);
    view.read_with(cx, |v, _| {
        let row = v.current_card().expect("a card").row;
        let ix = v.disp.iter().position(|d| d.id == row).expect("its row");
        let call = id_of(v, "ui-run-0").unwrap();
        assert_eq!(v.disp[ix - 1].item, v.model.transcript.items().iter().position(|i| i.id == call).unwrap(), "under the first call");
    });
}

/// Shady M6: a card placed above other rows that the list never drew is
/// not on screen (only the last row's first frame counts as shown): the
/// stub stays, and Esc does not deny a card nobody saw.
#[gpui_kit::test]
fn an_undrawn_card_above_other_rows_is_not_on_screen(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    resolve_prompt(cx, &view);
    wait(cx, w, &view, 1000);
    running_tools(cx, &view, 40);
    // The first call's confirm (the guard names it): its card far above the end.
    view.update(cx, |v, cx| {
        let request = serde_json::from_value::<synaps_client::protocol::PromptRequest>(serde_json::json!({
            "id": 40, "kind": "confirm", "title": "Confirm tool call",
            "prompt": "Extension: guard\nTool: bash\n\nMessage from guard:\nbash wants to run a command in ~/w:\n$ cargo test -p crate0\n\nAllow this tool call?",
            "confirm_scope": {"extensions": ["guard"], "tool": "bash"}
        }))
        .unwrap();
        v.model.transcript.apply(&SessionEvent::Prompt { request });
        cx.notify();
    });
    cx.run_until_parked();
    settle(cx, w, 8);
    view.read_with(cx, |v, _| {
        let ix = v.card_ix().expect("its row");
        assert!(ix + 1 < v.disp.len(), "rows under it");
        assert!(v.current_card().unwrap().height.get().is_nan(), "never drawn");
        assert!(!v.card_on_screen(), "not on screen");
    });
    assert!(VisualTestContext::from_window(w, cx).debug_bounds("needs-stub").is_some(), "the stub stands in");
    cx.update_window(w, |_, window, cx| {
        window.activate_window();
        view.update(cx, |v, cx| window.focus(&v.focus, cx))
    })
    .unwrap();
    settle(cx, w, 8);
    std::thread::sleep(crate::app::ARM + std::time::Duration::from_millis(60));
    tap(cx, w, "escape");
    assert!(view.read_with(cx, |v, _| v.sent.is_none()), "Esc denies nothing unseen");
}

/// Shady M5: Ctrl+J is for the card it was pressed for. That card gone,
/// the next one never takes the keys from the composer.
#[gpui_kit::test]
fn ctrl_j_is_for_one_card(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    view.update(cx, |v, cx| {
        v.jump_card = v.model.card_key().map(|(e, id)| (e, id + 1000));
        cx.notify();
    });
    settle(cx, w, 8);
    assert!(view.read_with(cx, |v, _| v.jump_card.is_none()), "another card's jump is dropped");
    view.update(cx, |v, _| v.jump_card = v.model.card_key());
    resolve_prompt(cx, &view);
    wait(cx, w, &view, 1000);
    assert!(view.read_with(cx, |v, _| v.jump_card.is_none()), "its card went: dropped");
    focus_composer(cx, w, &view);
    raise(cx, &view, 50);
    settle(cx, w, 8);
    assert!(composer_focused(cx, w, &view), "the next card leaves the composer its keys");
}

/// Motion Off: an answered card goes in the frame its answer lands.
#[gpui_kit::test]
fn the_answered_card_goes_at_once_with_motion_off(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Off);
    settle(cx, w, 8);
    resolve_prompt(cx, &view);
    assert!(view.read_with(cx, |v, _| v.card_exit.is_none()));
    assert!(VisualTestContext::from_window(w, cx).debug_bounds("card-exit").is_none());
}

/// A left-button press and release at `position`.
fn click_at(cx: &mut TestAppContext, w: AnyWindowHandle, position: gpui_kit::Point<gpui_kit::Pixels>) {
    use gpui_kit::{Modifiers, MouseButton, MouseDownEvent, MouseUpEvent};
    let mut vcx = VisualTestContext::from_window(w, cx);
    vcx.simulate_event(MouseDownEvent {
        button: MouseButton::Left,
        position,
        modifiers: Modifiers::default(),
        click_count: 1,
        first_mouse: false,
    });
    vcx.simulate_event(MouseUpEvent { button: MouseButton::Left, position, modifiers: Modifiers::default(), click_count: 1 });
    vcx.run_until_parked();
}

/// Shady H1: motion's memory is the shown column's and session's. Every
/// transcript numbers its rows from 1 (and every session its workers), so
/// another's row 12 is not this one's: a session switch in the same tab (a
/// rail click), a worker's view in and out, and back, all come up at rest:
/// no write-on, settle, warn draw, chevron turn, write head or Handoff
/// replayed from the other column, and no display frame asked for.
#[gpui_kit::test]
fn a_switch_replays_nothing(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    // The motion clock moves with the executor's only (Shady M4): whether a
    // switch is rapid never depends on how slow the runner is.
    view.read_with(cx, |v, _| v.fx.pin_clock());
    // `ms` on from the last frame's time (a timer the executor ran to may
    // have put that past the clock): frames this far apart, at least.
    let wait = |cx: &mut TestAppContext, ms: u64| {
        let d = std::time::Duration::from_millis(ms);
        view.read_with(cx, |v, _| v.fx.set_clock(v.fx.tick() + d));
        cx.executor().advance_clock(d);
        settle(cx, w, 8);
    };
    // The launch's column layer settles.
    wait(cx, 400);
    // Nothing changed after `t0` (Shady N3: a change started on the switch
    // and on any frame after it, not only the last frame's), no row grows,
    // and once the column's own layer (fluid.md §3.6, this view's, not a
    // replay) has settled, no display frame asked for.
    let mark = |cx: &mut TestAppContext| {
        wait(cx, 300);
        view.read_with(cx, |v, _| v.fx.clock())
    };
    let check = |cx: &mut TestAppContext, what: &str, t0: std::time::Instant| {
        let fresh = view.read_with(cx, |v, _| v.fx.fresh());
        assert!(fresh.is_empty(), "{what}: replayed {fresh:?}");
        // (The watched bar is this view's own: its draw is the one a view
        // opened or left here plays, as [`crate::motion::Motion::fresh`] has it.)
        let changed: Vec<_> =
            view.read_with(cx, |v, _| v.fx.changed_since(t0)).into_iter().filter(|(t, _)| *t != crate::motion::Tag::Watched).collect();
        assert!(changed.is_empty(), "{what}: replayed {changed:?}");
        assert_eq!(view.read_with(cx, |v, _| v.fx.growing()), 0, "{what}: a row grew");
        assert!(view.read_with(cx, |v, _| v.layer.is_some()), "{what}: the column arrives as one layer");
        wait(cx, 400);
        assert!(view.read_with(cx, |v, _| v.layer.is_none()), "{what}: the layer settled");
        assert!(!motion_pending(cx, &view).0, "{what}: a display frame asked for");
    };
    // A worker's view in place (its tray bar draws: that is this view's
    // own, not a replay), and out again.
    let t0 = mark(cx);
    scout_in_place(cx, w, &view);
    let fresh = view.read_with(cx, |v, _| v.fx.fresh());
    assert!(fresh.is_empty(), "scout's view in: replayed {fresh:?}");
    let changed: Vec<_> =
        view.read_with(cx, |v, _| v.fx.changed_since(t0)).into_iter().filter(|(t, _)| *t != crate::motion::Tag::Watched).collect();
    assert!(changed.is_empty(), "scout's view in: replayed {changed:?}");
    let t0 = mark(cx);
    view.update(cx, |v, cx| v.leave_worker(false, cx));
    cx.run_until_parked();
    check(cx, "scout's view out", t0);
    // relay-refresh → Idle CPU in the same tab (its rows share ids with
    // relay-refresh's; its turn is over, relay-refresh's foot was busy).
    view.update(cx, |v, _| v.client = None);
    let t0 = mark(cx);
    open_idle_cpu(cx, &view, crate::tabs::Place::Current);
    check(cx, "a switch in the tab", t0);
    only_the_rail_spins(cx, w, &view, "Idle CPU runs nothing");
    // And back.
    let t0 = mark(cx);
    view.update(cx, |v, cx| {
        v.open_tab(
            crate::app::Open::Attach { session_id: crate::demo::SID.into(), fallback: None, open: false },
            crate::tabs::Place::Current,
        );
        cx.notify();
    });
    cx.run_until_parked();
    assert_eq!(attached(cx, &view).as_deref(), Some(crate::demo::SID));
    check(cx, "back", t0);
}

/// `n` running bash calls in the shown session's transcript, on screen.
fn running_tools(cx: &mut TestAppContext, view: &Entity<AppView>, n: usize) {
    use synaps_client::protocol::{LlmEvent, StreamEvent};
    view.update(cx, |v, cx| {
        let t = &mut v.model.transcript;
        t.apply(&SessionEvent::Stream { event: StreamEvent::Llm(LlmEvent::ResponseStart) });
        for k in 0..n {
            let id = format!("ui-run-{k}");
            t.apply(&SessionEvent::Stream {
                event: StreamEvent::Llm(LlmEvent::ToolUseStart { tool_name: "bash".into(), tool_id: id.clone() }),
            });
            t.apply(&SessionEvent::Stream {
                event: StreamEvent::Llm(LlmEvent::ToolUse {
                    tool_name: "bash".into(),
                    tool_id: id,
                    input: serde_json::json!({"command": format!("cargo test -p crate{k}")}),
                }),
            });
        }
        v.sync_list();
        v.list.scroll_to_end();
        cx.notify();
    });
    cx.run_until_parked();
}

/// Cards D (cards4/proposal.md §3, an intended change to Q1's bare row):
/// a tool row sits in its run's card, the column wide less the card's
/// hairline and 3 px inset, 34 px tall, its life glyph inside it; its
/// AGENT label hangs left of the card. A failed call opens on its well at
/// the verb column (36 px in), to the row's padding.
#[gpui_kit::test]
fn a_tool_row_sits_in_its_runs_card(cx: &mut TestAppContext) {
    use synaps_client::protocol::{LlmEvent, StreamEvent};
    let (w, view, _events) = open_at(cx, (2200., 1400.), crate::theme::Panels::default().text_size);
    set_motion(cx, &view, crate::motion::Level::Full);
    // A turn of its own: its first row, the call, carries AGENT.
    view.update(cx, |v, _| {
        let n = v.model.transcript.items().len();
        v.model.transcript.apply(&SessionEvent::TurnStarted {
            turn_baseline: n as _,
            trigger: "user".into(),
            user_text: Some("Run it.".into()),
        });
    });
    running_tools(cx, &view, 1);
    for _ in 0..4 {
        beat(cx, w, &view);
    }
    let (id, lay, m) = view.read_with(cx, |v, cx| {
        let id = v.model.transcript.items().last().map(|i| i.id).expect("the call");
        (id, v.layout, crate::theme::metrics(cx))
    });
    assert!(lay.hang, "2200 px: the labels hang");
    let g = crate::cards::Geo::of(&m);
    let bounds = |cx: &mut TestAppContext, what: &'static str| {
        view.update(cx, |_, cx| cx.notify());
        cx.update_window(w, |_, window, cx| window.simulate_next_frame(cx)).expect("the window");
        cx.run_until_parked();
        VisualTestContext::from_window(w, cx).debug_bounds(what)
    };
    let row = bounds(cx, leak(format!("tool-row {id}"))).expect("the running call's row");
    assert_eq!(f32::from(row.size.width), lay.column - 2. - 2. * g.inset, "the column less the card's edge and inset");
    assert_eq!(row.size.height, gpui_kit::px(g.row), "one row");
    let status = bounds(cx, leak(format!("tool-status {id}"))).expect("the status glyph");
    assert!(f32::from(status.left()) >= f32::from(row.left()) + g.pad - 0.5 && status.right() <= row.right(), "{status:?}: inside the row");
    // Its AGENT label hangs left of the card.
    let label = bounds(cx, leak(format!("hang-label-{id}"))).expect("the call's AGENT label");
    assert!(label.right() <= row.left(), "the label hangs outside the card: {label:?} {row:?}");
    view.update(cx, |v, cx| {
        v.model.transcript.apply(&SessionEvent::Stream {
            event: StreamEvent::Llm(LlmEvent::ToolResult { tool_id: "ui-run-0".into(), result: "Tool execution failed: boom".into() }),
        });
        v.sync_list();
        v.list.scroll_to_end();
        cx.notify();
    });
    cx.run_until_parked();
    for _ in 0..4 {
        beat(cx, w, &view);
    }
    let failed = view.read_with(cx, |v, _| v.model.transcript.items().last().and_then(|i| i.tool.as_ref()).and_then(|t| t.status));
    assert_eq!(failed, Some(synaps_client::transcript::ToolStatus::Failed));
    let well = bounds(cx, leak(format!("well {id}"))).expect("a failed call opens on its well");
    let row = bounds(cx, leak(format!("tool-row {id}"))).expect("the row");
    assert_eq!(f32::from(well.left()), f32::from(row.left()) + g.indent, "the well at the verb column");
    assert_eq!(f32::from(well.right()), f32::from(row.right()) - g.pad, "to the row's padding");
    let status = bounds(cx, leak(format!("tool-status {id}"))).expect("its failed glyph");
    assert!(status.right() <= row.right() && status.left() >= row.left(), "still in its row");
}

/// One beat goes by: the motion clock and the executor's move on together,
/// a millisecond at a time (the one timer fires when it is due, as it
/// would), and what it drew settles. The motion clock is the test's
/// ([`open_with`]): only this moves it, a millisecond a step.
fn beat(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    let ms = std::time::Duration::from_millis(1);
    let t0 = view.read_with(cx, |v, _| v.fx.clock());
    for k in 1..=crate::motion::BEAT.as_millis() as u32 {
        view.read_with(cx, |v, _| v.fx.set_clock(t0 + ms * k));
        cx.executor().advance_clock(ms);
        cx.run_until_parked();
    }
    settle(cx, w, 4);
}

/// The app's frames (render, layout, prepaint) and the beat layer's paints.
fn frames(cx: &mut TestAppContext, view: &Entity<AppView>) -> (u64, u64) {
    view.read_with(cx, |v, _| (v.fx.frames(), v.fx.layer_paints.get()))
}

/// Motion.md §10 amendment 13: 25 running glyphs (and scout's) scan on the
/// beat, and a beat repaints only the beat layer over the app's cached
/// frame: the app's view is not rendered, laid out or prepainted. Its own
/// frames come only from the clocks' shared second (2 in 2 s), whatever
/// the number of clocks.
#[gpui_kit::test]
fn beats_repaint_only_the_layer(cx: &mut TestAppContext) {
    let (w, view, _events) = open_at(cx, (1440., 1800.), crate::theme::Panels::default().text_size);
    set_motion(cx, &view, crate::motion::Level::Full);
    running_tools(cx, &view, 25);
    // The arrivals draw in and settle.
    for _ in 0..12 {
        beat(cx, w, &view);
    }
    assert!(!motion_pending(cx, &view).0, "settled: no display frames");
    let sites = view.read_with(cx, |v, _| v.fx.sites());
    let scans = sites.iter().filter(|s| matches!(s.beat, crate::motion::Beat::Spin(_) | crate::motion::Beat::Lattice(_))).count();
    assert!(scans >= 25, "the running glyphs on screen are the layer's: {scans}");
    let (f0, l0) = frames(cx, &view);
    let p0 = view.read_with(cx, |v, _| v.fx.painted.borrow().len());
    // The timers armed (each once, however many beats it waits): the
    // lattices' pose changes (indicators/proposal.md §6: 4 a second) wake
    // only the layer; only the clocks' second wakes the app.
    let mut armed = Vec::new();
    for _ in 0..16 {
        let a = view.read_with(cx, |v, _| v.fx.pending().1.zip(v.fx.wakes()));
        if let Some(a) = a.filter(|a| !armed.contains(a)) {
            armed.push(a);
        }
        beat(cx, w, &view);
    }
    let (f1, l1) = frames(cx, &view);
    let layer_only = armed.iter().filter(|a| a.1 == crate::motion::Wake::Beat).count();
    // 2 s: the clocks' second twice (a third at most, as the edges fall).
    assert!(armed.len() - layer_only <= 3 && layer_only >= 5, "the timer woke only the layer on {layer_only} of {} wakes", armed.len());
    // Every beat paints it, once (Shady N2: a beat painted twice is the
    // regression the layer exists to prevent): a beat is painted again
    // only by an app frame inside it.
    assert_painted_once(cx, &view, p0, l1 - l0, f1 - f0);
    assert!(f1 - f0 <= 3, "16 beats drew the app {} times: only the clocks' second may", f1 - f0);
    // H2 (indicators/proposal.md §6): 25 running lattices stay within the
    // beat's budget (a paint at most a beat, one for all of them), and the
    // running calls' clocks still tick on the app's frame, each second.
    assert!(l1 - l0 <= 16, "{} layer paints in 16 beats", l1 - l0);
    assert!(f1 > f0, "the clocks' second drew the app");
    assert_eq!(view.read_with(cx, |v, _| v.fx.sites().len()), sites.len(), "the cached frame keeps its sites");
    // Off: the glyphs draw still in the app's frame; nothing for the layer.
    set_motion(cx, &view, crate::motion::Level::Off);
    assert!(view.read_with(cx, |v, _| v.fx.sites().is_empty()));
    assert_eq!(motion_pending(cx, &view), (false, None));
}

/// Finish gate fix 2: an armed cancel's fuse burns before CONFIRM, 8 px
/// from it, clear of the × the pointer armed it on (the armed row's right
/// end, where CONFIRM is); one cell going dark a second.
#[gpui_kit::test]
fn the_fuse_burns_clear_of_the_pointer(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    let scout = view.read_with(cx, |v, _| {
        let (call, name) = v.model.stats.subagents.ident(1).expect("sa_1 named");
        crate::wtabs::Who::new(1, name.to_string(), call.map(|c| c.to_string().into()))
    });
    view.update(cx, |v, cx| v.press_cancel(crate::demo::SID, &scout, crate::menu::How::Click(1), cx));
    cx.run_until_parked();
    settle(cx, w, 4);
    let b = |cx: &mut TestAppContext, sel: &'static str| VisualTestContext::from_window(w, cx).debug_bounds(sel).expect(sel);
    let (armed, fuse, confirm) = (b(cx, "tray-armed"), b(cx, "tray-fuse"), b(cx, "tray-confirm"));
    assert_eq!(f32::from(confirm.left() - fuse.right()), 8., "8 px before CONFIRM");
    assert!(f32::from(armed.right() - fuse.right()) >= 24., "clear of the ×'s end: {armed:?} {fuse:?}");
    assert!(confirm.right() <= armed.right() && fuse.left() >= armed.left());
    // Its chip holds all of it: CONFIRM is not clipped.
    let chip = b(cx, "chip 1");
    assert!(armed.right() <= chip.right() && confirm.right() <= chip.right(), "{confirm:?} in {chip:?}");
}

/// A local session's events (the bench's way), `raw` each.
fn local(cx: &mut TestAppContext, view: &Entity<AppView>, raws: Vec<serde_json::Value>) {
    view.update(cx, |v, cx| {
        let evs = raws
            .into_iter()
            .filter_map(|raw| {
                let event = synaps_client::protocol::decode_event(&raw).ok()?;
                Some(synaps_client::Event::session(crate::model::LOCAL_SEQ, "life", 1, event, std::sync::Arc::new(raw)))
            })
            .collect();
        v.apply_batch(evs, cx);
        v.sync_list();
        v.list.scroll_to_end();
        cx.notify();
    });
}

/// A worker's rows event: `tools` and `step`, running.
fn worker_rows(tools: u64, step: &str) -> serde_json::Value {
    serde_json::json!({"ev": "subagent_rows", "rows": [{"subagent_id": 1, "agent_name": "scout", "status": "running",
        "cancel_requested": false, "elapsed_secs": 2.0, "finished_elapsed": null, "step": step, "tools": tools}]})
}

/// What painted in the last app frame, in order (`Motion::paint_log`).
fn painted(cx: &mut TestAppContext, view: &Entity<AppView>) -> Vec<&'static str> {
    view.read_with(cx, |v, _| v.fx.paint_log.borrow().clone())
}

/// `a` painted, after `b` painted.
fn painted_after(order: &[&str], a: &str, b: &str) -> bool {
    matches!((order.iter().position(|o| *o == a), order.iter().position(|o| *o == b)), (Some(x), Some(y)) if x > y)
}

/// polish.md P5's word swap, in a window: a count whose words change with
/// it (`1 tool` → `2 tools`) keeps its old words until the digits cross
/// mid-roll, then the new ones; never `1 tools`. No count left rolling in
/// the app has a plural (worker rows change in place since Q3 r4; `+N
/// more`, NEEDS YOU and the rail chips have none; `N calls` never changes
/// while drawn), so a test-only count in the top bar
/// (`AppView::roll_probe`) rolls through the same element.
#[gpui_kit::test]
fn a_counts_words_swap_as_its_digits_cross(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    let probe = |cx: &mut TestAppContext, n: usize| {
        view.update(cx, |v, cx| {
            v.roll_probe = Some(n);
            cx.notify();
        });
    };
    probe(cx, 1);
    wait(cx, w, &view, 16);
    wait(cx, w, &view, 16);
    let k = crate::motion::ROLL_PROBE;
    let words =
        |cx: &mut TestAppContext| VisualTestContext::from_window(w, cx).debug_bounds(format!("rolled-words-{k:x}").leak()).is_some();
    let rolling = |cx: &mut TestAppContext| !view.read_with(cx, |v, _| v.fx.rolling_in(Some(k))).is_empty();
    assert!(!rolling(cx) && !words(cx), "at rest");
    probe(cx, 2);
    wait(cx, w, &view, 16);
    assert!(rolling(cx), "`1 tool` → `2 tools` rolls");
    assert!(words(cx), "the first frames: `1 tool`, the digit leaving");
    let mut crossed = false;
    for _ in 0..40 {
        wait(cx, w, &view, 16);
        crossed |= rolling(cx) && !words(cx);
        assert!(!crossed || !words(cx), "the old word never comes back");
    }
    assert!(crossed, "mid-roll: `2 tools`, the digit still arriving");
    assert!(!rolling(cx) && !words(cx), "landed: one text");
}

/// polish.md P5: a count that still rolls (the top bar's NEEDS YOU, one
/// more session waiting) rolls only its changed digit cells, on the frame
/// after it changes: the new text and the old a line apart, in px, on
/// screen, constant as they travel, then one text at rest. Its words are
/// the same (`N   NEED YOU`): no old words are drawn (`rolled-words`).
#[gpui_kit::test]
fn a_count_rolls_in_its_digit_cells(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    settle(cx, w, 8);
    let rolling = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.fx.rolling_in(Some(crate::motion::ROLL_NEEDS)));
    assert!(rolling(cx).is_empty(), "at rest");
    view.update(cx, |v, cx| {
        let id = v
            .model
            .sessions
            .iter()
            .find(|m| m.awaiting_input == 0 && m.lifecycle != "ending" && !v.model.is_active(&m.id))
            .map(|m| m.id.clone())
            .expect("a session not waiting");
        if let Some(m) = v.model.sessions.iter_mut().find(|m| m.id == id) {
            m.awaiting_input = 1;
        }
        v.model.rail_gen += 1;
        cx.notify();
    });
    wait(cx, w, &view, 16);
    let keys = rolling(cx);
    assert_eq!(keys.len(), 1, "NEEDS YOU's count rolls");
    let k = keys[0];
    let apart = |cx: &mut TestAppContext| {
        let mut vcx = VisualTestContext::from_window(w, cx);
        let new = vcx.debug_bounds(format!("rolling-{k:x}").leak()).expect("the new text drawn");
        let old = vcx.debug_bounds(format!("rolled-{k:x}").leak()).expect("the old text drawn");
        f32::from(new.origin.y) - f32::from(old.origin.y)
    };
    let d0 = apart(cx);
    assert!(d0 > 8., "a line apart, the new one coming up from below: {d0}");
    let words =
        |cx: &mut TestAppContext| VisualTestContext::from_window(w, cx).debug_bounds(format!("rolled-words-{k:x}").leak()).is_some();
    assert!(!words(cx), "the words are the same: drawn once, in place");
    for _ in 0..3 {
        wait(cx, w, &view, 16);
    }
    let d1 = apart(cx);
    assert!(d1 > 0. && (d1 - d0).abs() < 0.01, "a line apart all the way: {d0} {d1}");
    for _ in 0..40 {
        wait(cx, w, &view, 16);
    }
    assert!(rolling(cx).is_empty(), "landed");
    assert!(VisualTestContext::from_window(w, cx).debug_bounds(format!("rolling-{k:x}").leak()).is_none(), "at rest: one text");
}

/// Q3 r4 (polish.md P5, gate 4 under a live fan-out; the design's final):
/// a spawn row's tool count and its live step swap in one frame, at every
/// level, with no roll and no fade. The frame they change on draws them,
/// nothing rolls, no display frame is asked for, and no frames follow for
/// them (a roll was 30 Hz of app frames for ~200 ms): at most a clock's
/// second (none under Off, where the clocks hold).
#[gpui_kit::test]
fn a_spawn_rows_count_and_step_swap_in_one_frame(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    life_worker(cx, w, &view);
    let levels = [crate::motion::Level::Full, crate::motion::Level::Reduced, crate::motion::Level::Off];
    for (k, level) in levels.into_iter().enumerate() {
        set_motion(cx, &view, level);
        settle(cx, w, 8);
        let f0 = frames(cx, &view).0;
        local(cx, &view, vec![worker_rows(2 + k as u64, ["grep -n rotate src", "reading src/b.rs", "cargo check"][k])]);
        wait(cx, w, &view, 16);
        let f1 = frames(cx, &view).0;
        assert!(f1 > f0, "{level:?}: the change drew");
        assert_eq!(view.read_with(cx, |v, _| v.fx.rolling_in(None)), Vec::<u64>::new(), "{level:?}: nothing rolls");
        assert!(!motion_pending(cx, &view).0, "{level:?}: no display frame asked for");
        for _ in 0..18 {
            wait(cx, w, &view, 16);
        }
        let more = frames(cx, &view).0 - f1;
        assert!(more <= 1, "{level:?}: no frames for it, at most a clock's second: {more}");
    }
}

/// polish.md P5: clocks, cost, tokens and % never roll. A running worker's
/// clocks (its spawn row's and its tray chip's) tick for three seconds:
/// every frame they draw rolls nothing.
#[gpui_kit::test]
fn clocks_never_roll(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    life_worker(cx, w, &view);
    let f0 = frames(cx, &view).0;
    for _ in 0..30 {
        wait(cx, w, &view, 100);
        assert_eq!(view.read_with(cx, |v, _| v.fx.rolling_in(None)), Vec::<u64>::new(), "a clock's second rolled");
    }
    assert!(frames(cx, &view).0 >= f0 + 3, "the clocks ticked (their frames)");
}

/// The bench's life scene's worker (`SYNAPS_BENCH_LIFE`): a local session,
/// a turn, scout spawned and running (`1 tool`, its step), at rest.
fn life_worker(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    view.update(cx, |v, cx| {
        v.client = None;
        v.model.set_local_session("life");
        let t = &mut v.model.transcript;
        t.apply(&SessionEvent::TurnStarted { turn_baseline: 0, trigger: "user".into(), user_text: Some("go".into()) });
        v.sync_list();
        cx.notify();
    });
    let res = serde_json::json!({"handle_id": "sa_1", "agent_name": "scout", "status": "running"}).to_string();
    local(
        cx,
        view,
        vec![
            serde_json::json!({"ev": "stream", "event": {"kind": "llm", "llm": "tool_use", "tool_name": "subagent_start",
                "tool_id": "w1", "input": {"agent": "scout", "task": "find the callers"}}}),
            serde_json::json!({"ev": "stream", "event": {"kind": "llm", "llm": "tool_result", "tool_id": "w1", "result": res}}),
            worker_rows(1, "reading src/a.rs"),
        ],
    );
    // Two seconds: its clock ticks (the app draws) in between.
    for _ in 0..25 {
        wait(cx, w, view, 100);
    }
}

/// Finish gate fix 1: a turn that ends here hands the floor back: Handoff
/// in's light travels down the composer's bar and is gone after `sig`
/// (its colour, decaying into the bar: `motion::handoff_in_decays_into_the_bar`);
/// Off draws none. P3 (polish.md, settle_pass_once_per_turn_end): the
/// composer's top edge makes one pass from the same corner, once; none
/// under Reduced or Off. Cards D (an intended change): P2's sweep is
/// retired; while the turn runs, the state cell laps the orbit.
#[gpui_kit::test]
fn handoff_in_travels_and_goes(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    let client = view.update(cx, |v, _| v.client.take());
    open_idle_cpu(cx, &view, crate::tabs::Place::Current);
    let busy = |cx: &mut TestAppContext, b: bool| {
        view.update(cx, |v, cx| {
            v.model.transcript.set_busy(b);
            cx.notify();
        });
        cx.run_until_parked();
    };
    view.update(cx, |v, _| {
        v.client = client;
        // Live (the demo's is parked): its turn's end says READY.
        let m = v.model.sessions.iter_mut().find(|m| m.id == crate::demo::IDLE_CPU).expect("listed");
        m.lifecycle = "live".into();
    });
    busy(cx, true);
    for _ in 0..4 {
        beat(cx, w, &view);
    }
    let seg = |cx: &mut TestAppContext| VisualTestContext::from_window(w, cx).debug_bounds("handoff-in");
    // polish.md P3: the composer's state cell laps the orbit while the turn
    // runs (no sweep: Cards D), then its top edge makes one pass with the
    // Handoff light, from its corner.
    let pass = |cx: &mut TestAppContext| VisualTestContext::from_window(w, cx).debug_bounds("settle-pass");
    let sweep = |cx: &mut TestAppContext| VisualTestContext::from_window(w, cx).debug_bounds("sweep");
    let orbit = |cx: &mut TestAppContext| VisualTestContext::from_window(w, cx).debug_bounds("foot-lattice");
    assert!(seg(cx).is_none());
    assert!(orbit(cx).is_some() && sweep(cx).is_none() && pass(cx).is_none(), "running: the state cell's orbit, no sweep");
    busy(cx, false);
    assert_eq!(view.read_with(cx, |v, _| v.model.tabs.current().map(|t| v.tab_state(t))), Some(crate::rail::State::Idle), "READY");
    assert!(seg(cx).is_some(), "the turn ended here: the light travels down the bar");
    let (s, p) = (seg(cx).unwrap(), pass(cx).expect("settle_pass_once_per_turn_end: the pass with the light"));
    assert!(orbit(cx).is_none(), "the orbit is over");
    assert!((f32::from(p.origin.y) - f32::from(s.origin.y)).abs() <= 1., "from the same corner: {p:?} / {s:?}");
    // Shady #17 R3-M2: the pass lies on the composer's top border, which
    // GPUI paints after the composer's children: drawn among them, the
    // border covered it (4c63a0c; `debug_bounds` above still found it). It
    // paints after the root's last child, so over every border.
    let order = painted(cx, &view);
    assert!(painted_after(&order, "settle-pass", "end-frame"), "the pass paints over the composer's border: {order:?}");
    // P15: the sticky layer paints with the list, under the composer and
    // under the deferred pass.
    assert!(painted_after(&order, "composer", "sticky") && painted_after(&order, "settle-pass", "sticky"), "{order:?}");
    for _ in 0..6 {
        beat(cx, w, &view);
    }
    assert!(seg(cx).is_none(), "gone after sig");
    assert!(pass(cx).is_none(), "one pass, then everything stops");
    // Nothing of it still animates (the design's final): no display frame,
    // no orbit, and what is left on the layer is the rail's other sessions'
    // spinners; with those at rest, nothing at all.
    only_the_rail_spins(cx, w, &view, "after the pass (Full)");
    set_motion(cx, &view, crate::motion::Level::Reduced);
    busy(cx, true);
    assert!(sweep(cx).is_none(), "Reduced: the edge is unlit");
    assert!(view.read_with(cx, |v, _| v.fx.lattice_count()) == 0, "Reduced: the lattices are still, off the layer");
    busy(cx, false);
    assert!(pass(cx).is_none(), "Reduced: no pass (the bar's flash)");
    set_motion(cx, &view, crate::motion::Level::Off);
    busy(cx, true);
    busy(cx, false);
    assert!(seg(cx).is_none(), "Off: none");
    assert!(pass(cx).is_none(), "Off: no pass");
    assert_eq!(motion_pending(cx, &view), (false, None));
}

/// The other demo session (no worker, so no clock) with `n` calls running,
/// their arrivals settled: what runs is only the beat.
fn running_without_a_clock(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, n: usize) {
    view.update(cx, |v, _| v.client = None);
    open_idle_cpu(cx, view, crate::tabs::Place::Current);
    set_motion(cx, view, crate::motion::Level::Full);
    running_tools(cx, view, n);
    for _ in 0..12 {
        beat(cx, w, view);
    }
    let scans = view.read_with(cx, |v, _| {
        v.fx.sites().iter().filter(|s| matches!(s.beat, crate::motion::Beat::Spin(_) | crate::motion::Beat::Lattice(_))).count()
    });
    assert!(scans >= n, "the running glyphs are the layer's: {scans}");
    assert!(view.read_with(cx, |v, _| v.tray.rows.is_empty()), "no worker: no clock");
}

/// Shady N2: with no clock on screen, 16 beats draw the app's view not
/// once: each is one paint of the layer.
#[gpui_kit::test]
fn beats_without_a_clock_draw_no_app_frame(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    running_without_a_clock(cx, w, &view, 3);
    let (f0, l0) = frames(cx, &view);
    let p0 = view.read_with(cx, |v, _| v.fx.painted.borrow().len());
    for _ in 0..16 {
        assert_eq!(view.read_with(cx, |v, _| v.fx.wakes()), Some(crate::motion::Wake::Beat), "the timer wakes the layer");
        beat(cx, w, &view);
    }
    let (f1, l1) = frames(cx, &view);
    assert_eq!(f1, f0, "16 beats drew the app's view {} times", f1 - f0);
    assert_painted_once(cx, &view, p0, l1 - l0, 0);
}

/// The `paints` the layer made since its `p0`th: each a beat of its own
/// (at most `again` repainted, by the app's own frames), in order, about
/// one a test beat (a slow runner's wall time can carry the clock past a
/// beat, and then its paints are for later beats, never the same one).
fn assert_painted_once(cx: &mut TestAppContext, view: &Entity<AppView>, p0: usize, paints: u64, again: u64) {
    let painted = view.read_with(cx, |v, _| v.fx.painted.borrow()[p0..].to_vec());
    assert_eq!(painted.len() as u64, paints, "every paint had the beat's sites to paint");
    assert!(painted.windows(2).all(|w| w[1] >= w[0]), "in order: {painted:?}");
    let twice = painted.windows(2).filter(|w| w[1] == w[0]).count() as u64;
    assert!(twice <= again, "{twice} beats painted twice ({again} app frames): {painted:?}");
    // The lattice (indicators/proposal.md §6) repaints where a pose
    // changes: running, twice a 4-beat step.
    assert!(painted.len() >= 7, "a paint a pose change: {painted:?}");
}

/// Shady N1: low power stops the beat (§7: no ambient). GPUI stores a
/// power change without a refresh; the layer's next beat sees it, draws
/// the app's view instead (its glyphs still, no sites) and arms nothing.
#[gpui_kit::test]
fn low_power_stops_the_beat_layer(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    running_without_a_clock(cx, w, &view, 3);
    // Battery saver: only the field changes, as GPUI's handler does.
    view.read_with(cx, |v, _| v.fx.force_low_power.set(true));
    let f0 = frames(cx, &view).0;
    beat(cx, w, &view);
    assert!(frames(cx, &view).0 > f0, "the layer drew the app's view");
    assert!(view.read_with(cx, |v, _| v.fx.sites().is_empty()), "drawn still: nothing on the layer");
    assert_eq!(motion_pending(cx, &view), (false, None), "no beat armed, no frame asked for");
    let (f1, l1) = frames(cx, &view);
    for _ in 0..8 {
        beat(cx, w, &view);
    }
    assert_eq!(frames(cx, &view), (f1, l1), "a second of beats: nothing drawn, nothing painted");
    // Out of low power, the next frame scans again.
    view.read_with(cx, |v, _| v.fx.force_low_power.set(false));
    frame(cx, &view);
    assert!(!view.read_with(cx, |v, _| v.fx.sites().is_empty()), "on the layer again");
    assert_eq!(view.read_with(cx, |v, _| v.fx.wakes()), Some(crate::motion::Wake::Beat));
}

/// Shady N6: the beat in the app's frame (`SYNAPS_DESKTOP_BEAT_IN_FRAME=1`,
/// the measuring path, as before the layer): no sites; each beat is an app
/// frame, and the running glyph's braille frame is drawn in it, the
/// frame's own beat's (polish.md P1).
#[gpui_kit::test]
fn the_beat_in_the_frame_spins_without_the_layer(cx: &mut TestAppContext) {
    let (w, view, _events) = open_with(cx, (1440., 900.), crate::theme::Panels::default().text_size, false);
    running_without_a_clock_in_frame(cx, w, &view);
    let at = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.fx.frame_beat());
    let drawn = |cx: &mut TestAppContext, k: Option<u32>| {
        let pose = crate::lattice::pose(crate::lattice::Pat::Run, k);
        VisualTestContext::from_window(w, cx).debug_bounds(format!("lattice-{:07x}", crate::lattice::code(pose)).leak()).is_some()
    };
    let (f0, k0) = (frames(cx, &view).0, at(cx));
    assert!(drawn(cx, Some(k0)), "the pose of beat {k0} in the frame");
    assert_eq!(view.read_with(cx, |v, _| v.fx.wakes()), Some(crate::motion::Wake::App), "the timer wakes the app");
    // The lattice's next pose change (indicators/proposal.md §6: at most
    // a step's 4 beats away) is the app's frame.
    for _ in 0..4 {
        beat(cx, w, &view);
        if frames(cx, &view).0 > f0 {
            break;
        }
    }
    let (f1, k1) = (frames(cx, &view).0, at(cx));
    assert!(f1 > f0 && k1 > k0, "the pose change is the app's frame");
    assert!(view.read_with(cx, |v, _| v.fx.sites().is_empty()), "nothing on the layer");
    assert!(drawn(cx, Some(k1)), "the pose of beat {k1}: the face moved on");
    // (Cards D retired the composer's sweep, which this checked painted
    // over the composer's border: the pass's order is
    // `handoff_in_travels_and_goes`'.)
    // Off: still, the running still pose.
    set_motion(cx, &view, crate::motion::Level::Off);
    assert!(drawn(cx, None));
    assert_eq!(motion_pending(cx, &view), (false, None));
}

/// Cards D (step 8, replacing P2's `a_sweep_ticks_only_while_drawn`), H2
/// (indicators/proposal.md §8): the composer's state cell is the cards'
/// lattice while the turn runs, on the beat layer, its one timer at the
/// next pose change on the beat grid; Reduced draws it still, and the
/// timer goes to a clock's second or nothing.
#[gpui_kit::test]
fn the_composers_lattice_steps_only_while_drawn(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    // A session with no prompt waiting (the demo's NEEDS YOU stills the edge).
    view.update(cx, |v, _| v.client = None);
    open_idle_cpu(cx, &view, crate::tabs::Place::Current);
    running_tools(cx, &view, 1);
    view.update(cx, |v, cx| {
        v.model.transcript.set_busy(true);
        cx.notify();
    });
    for _ in 0..12 {
        beat(cx, w, &view);
    }
    let cards = |cx: &mut TestAppContext| {
        view.read_with(cx, |v, _| v.fx.sites().iter().filter(|s| matches!(s.beat, crate::motion::Beat::Lattice(_))).count())
    };
    assert!(cards(cx) >= 2, "the turn runs: the call's lattice and the composer's");
    assert!(VisualTestContext::from_window(w, cx).debug_bounds("foot-lattice").is_some());
    let (frame, at) = motion_pending(cx, &view);
    let ahead = view.read_with(cx, |v, _| at.map(|t| t.saturating_duration_since(v.fx.clock())));
    assert!(!frame && ahead.is_some_and(|d| d <= crate::motion::BEAT * crate::lattice::STEP_BEATS), "the next pose: {ahead:?}");
    assert!(view.read_with(cx, |v, _| at.is_some_and(|t| v.fx.on_beats(t, 1))), "on the beat grid");
    assert_eq!(view.read_with(cx, |v, _| v.fx.wakes()), Some(crate::motion::Wake::Beat), "the layer, not the app");
    set_motion(cx, &view, crate::motion::Level::Reduced);
    assert_eq!(cards(cx), 0, "Reduced: the lattices are still");
    settle(cx, w, 4);
    let (frame, at) = motion_pending(cx, &view);
    assert!(!frame);
    assert!(at.is_none_or(|t| view.read_with(cx, |v, _| v.fx.on_beats(t, 8))), "only a clock's second, or nothing");
}

/// H2 (indicators/proposal.md §6, §8): a call finishing while watched
/// turns its lattice a quarter (+45° from its running pose, then +90°),
/// then its dots go out under the tick writing on; with the switch off
/// (H1, the fallback) the tick writes on over the dots going out at once,
/// no turn. Both land on the tick at rest.
#[gpui_kit::test]
fn a_finished_call_turns_its_lattice_or_not(cx: &mut TestAppContext) {
    use synaps_client::protocol::{LlmEvent, StreamEvent};
    for turn in [true, false] {
        let (w, view, _events) = open(cx);
        set_motion(cx, &view, crate::motion::Level::Full);
        view.update(cx, |v, _| {
            v.client = None;
            v.fx.turn_on_change.set(turn);
        });
        running_tools(cx, &view, 1);
        for _ in 0..4 {
            beat(cx, w, &view);
        }
        let id = view.read_with(cx, |v, _| v.model.transcript.items().last().map(|i| i.id)).expect("the call");
        let has = |cx: &mut TestAppContext, sel: String| VisualTestContext::from_window(w, cx).debug_bounds(String::leak(sel)).is_some();
        assert!(has(cx, format!("tool-status {id}")), "{turn}: its glyph");
        view.update(cx, |v, cx| {
            v.model.transcript.apply(&SessionEvent::Stream {
                event: StreamEvent::Llm(LlmEvent::ToolResult { tool_id: "ui-run-0".into(), result: "ok".into() }),
            });
            v.sync_list();
            cx.notify();
        });
        cx.run_until_parked();
        let mut seen = Vec::new();
        for _ in 0..40 {
            fluid_frame(cx, w, &view);
            for sel in ["life-turn 0", "life-turn 1", "life-settle Done"] {
                if !seen.contains(&sel) && has(cx, sel.to_string()) {
                    seen.push(sel);
                }
            }
        }
        if turn {
            assert_eq!(seen, ["life-turn 0", "life-turn 1", "life-settle Done"], "H2: the turn, then the tick");
        } else {
            assert_eq!(seen, ["life-settle Done"], "H1: the tick at once, no turn");
        }
        // At rest: the tick, nothing asked for.
        wait(cx, w, &view, 1000);
        assert!(has(cx, "life-Done".to_string()), "{turn}: at rest");
    }
}

/// The finish gate of #23, item 1 (indicators/proposal.md §8): a call's
/// body opens and closes under its row on `glide`: the frame after the
/// toggle shows less than all of it, `glide` later all of it (or none).
#[gpui_kit::test]
fn a_call_body_reveals_on_glide(cx: &mut TestAppContext) {
    use synaps_client::protocol::{LlmEvent, StreamEvent};
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    view.update(cx, |v, _| v.client = None);
    running_tools(cx, &view, 1);
    view.update(cx, |v, cx| {
        v.model.transcript.apply(&SessionEvent::Stream {
            event: StreamEvent::Llm(LlmEvent::ToolResult { tool_id: "ui-run-0".into(), result: "line one\nline two\nline three".into() }),
        });
        v.sync_list();
        v.list.scroll_to_end();
        cx.notify();
    });
    wait(cx, w, &view, 1000);
    let id = view.read_with(cx, |v, _| v.model.transcript.items().last().map(|i| i.id)).expect("the call");
    let b = |cx: &mut TestAppContext, sel: String| VisualTestContext::from_window(w, cx).debug_bounds(String::leak(sel));
    assert!(b(cx, format!("reveal {id}")).is_none(), "a done call's body is closed");
    let toggle = |cx: &mut TestAppContext| {
        view.update(cx, |v, cx| {
            v.toggle(id);
            cx.notify();
        });
        cx.run_until_parked();
    };
    toggle(cx);
    let mut first = None;
    for _ in 0..3 {
        fluid_frame(cx, w, &view);
        if let Some(r) = b(cx, format!("reveal {id}")).filter(|r| r.size.height > gpui_kit::px(0.)) {
            first.get_or_insert(r.size.height);
        }
    }
    let well = b(cx, format!("well {id}")).expect("its well, laid out whole");
    let first = first.expect("the body showing");
    assert!(first < well.size.height, "opening: {first:?} of {:?}", well.size.height);
    for _ in 0..80 {
        fluid_frame(cx, w, &view);
    }
    let open = b(cx, format!("reveal {id}")).expect("open");
    assert!(open.size.height >= well.size.height, "open: all of it ({open:?}, {well:?})");
    toggle(cx);
    fluid_frame(cx, w, &view);
    fluid_frame(cx, w, &view);
    let closing = b(cx, format!("reveal {id}")).expect("closing: still drawn");
    assert!(closing.size.height < open.size.height && closing.size.height > gpui_kit::px(0.), "closing: {closing:?}");
    for _ in 0..80 {
        fluid_frame(cx, w, &view);
    }
    assert!(b(cx, format!("reveal {id}")).is_none(), "closed");
    // Off: at once.
    set_motion(cx, &view, crate::motion::Level::Off);
    toggle(cx);
    let at_once = b(cx, format!("reveal {id}")).expect("Off: open at once");
    assert_eq!(at_once.size.height, open.size.height);
}

/// The finish gate of #23, item 2 (cards4/proposal.md §8 #4): a running
/// call's output streams into its open body a line at a time, each new
/// line's clip growing from nothing to its line on `flick`.
#[gpui_kit::test]
fn running_output_grows_a_line_at_a_time(cx: &mut TestAppContext) {
    use synaps_client::protocol::{LlmEvent, StreamEvent};
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    view.update(cx, |v, _| v.client = None);
    running_tools(cx, &view, 1);
    let delta = |cx: &mut TestAppContext, text: &str| {
        view.update(cx, |v, cx| {
            v.model.transcript.apply(&SessionEvent::Stream {
                event: StreamEvent::Llm(LlmEvent::ToolResultDelta { tool_id: "ui-run-0".into(), delta: text.into() }),
            });
            v.sync_list();
            v.list.scroll_to_end();
            cx.notify();
        });
        cx.run_until_parked();
    };
    delta(cx, "running 3 tests\n");
    wait(cx, w, &view, 600);
    let id = view.read_with(cx, |v, _| v.model.transcript.items().last().map(|i| i.id)).expect("the call");
    let b = |cx: &mut TestAppContext, sel: String| VisualTestContext::from_window(w, cx).debug_bounds(String::leak(sel));
    assert!(b(cx, format!("well {id}")).is_some(), "a running call with output opens on it");
    let r = cx.update(|cx| crate::theme::metrics(cx).r);
    let line = gpui_kit::px((19. * r).round());
    delta(cx, "test a ... ok\n");
    let mut heights = Vec::new();
    for _ in 0..40 {
        fluid_frame(cx, w, &view);
        heights.push(b(cx, format!("stream-line {id} 1")).map(|l| l.size.height));
    }
    let grown: Vec<_> = heights.iter().flatten().collect();
    assert!(!grown.is_empty(), "the new line grows in its clip");
    assert!(*grown[0] < line, "it starts short of its line: {heights:?}");
    assert!(grown.windows(2).all(|w| w[1] >= w[0]), "and only grows: {heights:?}");
    assert!(heights.last().unwrap().is_none_or(|h| h == line), "then it is a plain line: {heights:?}");
    // Off: the line is there at once, no clip.
    set_motion(cx, &view, crate::motion::Level::Off);
    delta(cx, "test b ... ok\n");
    assert!(b(cx, format!("stream-line {id} 2")).is_none(), "Off: no growth");
}

/// The finish gate of #23, item 5 (indicators/proposal.md §8): a call that
/// fails while watched never draws a blank glyph: through the turn (H2) or
/// at once (H1), the old dots, the ring or the cross is on every frame.
#[gpui_kit::test]
fn a_failed_glyph_is_never_blank(cx: &mut TestAppContext) {
    use synaps_client::protocol::{LlmEvent, StreamEvent};
    for turn in [true, false] {
        let (w, view, _events) = open(cx);
        set_motion(cx, &view, crate::motion::Level::Full);
        view.update(cx, |v, _| {
            v.client = None;
            v.fx.turn_on_change.set(turn);
        });
        running_tools(cx, &view, 1);
        for _ in 0..4 {
            beat(cx, w, &view);
        }
        view.update(cx, |v, cx| {
            v.model.transcript.apply(&SessionEvent::Stream {
                event: StreamEvent::Llm(LlmEvent::ToolResult {
                    tool_id: "ui-run-0".into(),
                    result: "Tool execution failed: Command failed (exit 2):\nboom".into(),
                }),
            });
            v.sync_list();
            cx.notify();
        });
        cx.run_until_parked();
        let has = |cx: &mut TestAppContext, sel: &'static str| VisualTestContext::from_window(w, cx).debug_bounds(sel).is_some();
        let mut changing = 0;
        for k in 0..60 {
            fluid_frame(cx, w, &view);
            assert!(!has(cx, "ink-0"), "{turn}: frame {k} draws nothing");
            changing += usize::from(has(cx, "ink-1"));
        }
        assert!(changing > 10, "{turn}: it changed while watched ({changing} frames)");
        assert!(has(cx, "life-Failed"), "{turn}: the cross at rest");
    }
}

/// The finish gate of #23 (states-full f542): a streaming call that fails
/// holds its body open, and its height glides from the streamed lines to
/// the failure's, never a jump that moves the column in one frame.
#[gpui_kit::test]
fn a_failed_body_glides_to_its_output(cx: &mut TestAppContext) {
    use synaps_client::protocol::{LlmEvent, StreamEvent};
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    view.update(cx, |v, _| v.client = None);
    running_tools(cx, &view, 1);
    let send = |cx: &mut TestAppContext, e: LlmEvent| {
        view.update(cx, |v, cx| {
            v.model.transcript.apply(&SessionEvent::Stream { event: StreamEvent::Llm(e) });
            v.sync_list();
            v.list.scroll_to_end();
            cx.notify();
        });
        cx.run_until_parked();
    };
    let lines: String = (0..8).map(|k| format!("test t{k} ... ok\n")).collect();
    send(cx, LlmEvent::ToolResultDelta { tool_id: "ui-run-0".into(), delta: lines });
    wait(cx, w, &view, 1500);
    let id = view.read_with(cx, |v, _| v.model.transcript.items().last().map(|i| i.id)).expect("the call");
    let b = |cx: &mut TestAppContext, sel: String| VisualTestContext::from_window(w, cx).debug_bounds(String::leak(sel));
    let before = b(cx, format!("well {id}")).expect("streaming: open").size.height;
    send(cx, LlmEvent::ToolResult { tool_id: "ui-run-0".into(), result: "Tool execution failed: Command failed (exit 1):\nboom".into() });
    let mut heights = Vec::new();
    for _ in 0..60 {
        fluid_frame(cx, w, &view);
        heights.push(b(cx, format!("reveal {id}")).map(|r| f32::from(r.size.height)));
    }
    let after = b(cx, format!("well {id}")).expect("failed: still open").size.height;
    assert!(after < before, "the failure's output is shorter: {after:?} < {before:?}");
    let h: Vec<f32> = heights.into_iter().flatten().collect();
    assert!(h.len() > 5, "it glided over frames: {h:?}");
    assert!(h[0] >= f32::from(before) - 1., "its first frame holds the height it had: {h:?} ({before:?})");
    let step = h.windows(2).map(|p| (p[0] - p[1]).abs()).fold(0f32, f32::max);
    assert!(step < f32::from(before - after) * 0.6, "no one-frame jump: {h:?}");
}

/// Shady #23 round 2 M1: what the user makes a call's body stays what
/// they see while its default moves under it. Closed while its output
/// streams, it stays closed when the call succeeds (its default goes from
/// open to closed: a toggle against the old default must not open it).
/// Closed and opened again, it stays open past its success.
#[gpui_kit::test]
fn a_call_the_user_closed_stays_closed_when_it_succeeds(cx: &mut TestAppContext) {
    use synaps_client::protocol::{LlmEvent, StreamEvent};
    for reopen in [false, true] {
        let (w, view, _events) = open(cx);
        set_motion(cx, &view, crate::motion::Level::Full);
        view.update(cx, |v, _| v.client = None);
        running_tools(cx, &view, 1);
        let send = |cx: &mut TestAppContext, e: LlmEvent| {
            view.update(cx, |v, cx| {
                v.model.transcript.apply(&SessionEvent::Stream { event: StreamEvent::Llm(e) });
                v.sync_list();
                v.list.scroll_to_end();
                cx.notify();
            });
            cx.run_until_parked();
        };
        send(cx, LlmEvent::ToolStarted { tool_id: "ui-run-0".into() });
        send(cx, LlmEvent::ToolResultDelta { tool_id: "ui-run-0".into(), delta: "compiling\nlinking\n".into() });
        wait(cx, w, &view, 800);
        let id = view.read_with(cx, |v, _| v.model.transcript.items().last().map(|i| i.id)).expect("the call");
        let b = |cx: &mut TestAppContext, sel: String| VisualTestContext::from_window(w, cx).debug_bounds(String::leak(sel));
        assert!(b(cx, format!("well {id}")).is_some(), "{reopen}: its output streams open");
        let toggle = |cx: &mut TestAppContext| {
            view.update(cx, |v, cx| {
                v.toggle(id);
                cx.notify();
            });
            cx.run_until_parked();
            wait(cx, w, &view, 1000);
        };
        toggle(cx);
        assert!(b(cx, format!("well {id}")).is_none(), "{reopen}: the user closed it");
        if reopen {
            toggle(cx);
            assert!(b(cx, format!("well {id}")).is_some(), "opened again");
        }
        send(cx, LlmEvent::ToolResult { tool_id: "ui-run-0".into(), result: "compiling\nlinking\nok".into() });
        for k in 0..60 {
            fluid_frame(cx, w, &view);
            let shown = b(cx, format!("well {id}")).is_some();
            assert_eq!(shown, reopen, "{reopen}: frame {k} after its success, as the user left it");
        }
    }
}

/// Shady #23 round 2 M2: the composer's state cell streams only while text
/// arrives (prose being written, or a call's output coming in) and runs
/// while a call runs without output, as the call's own row does. A call is
/// `streaming` from its start to its result: that alone is not writing.
#[gpui_kit::test]
fn the_composer_streams_only_while_text_arrives(cx: &mut TestAppContext) {
    use synaps_client::protocol::{LlmEvent, StreamEvent};
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    view.update(cx, |v, _| v.client = None);
    open_idle_cpu(cx, &view, crate::tabs::Place::Current);
    running_tools(cx, &view, 1);
    view.update(cx, |v, cx| {
        v.model.transcript.set_busy(true);
        cx.notify();
    });
    let send = |cx: &mut TestAppContext, e: LlmEvent| {
        view.update(cx, |v, cx| {
            v.model.transcript.apply(&SessionEvent::Stream { event: StreamEvent::Llm(e) });
            v.sync_list();
            v.list.scroll_to_end();
            cx.notify();
        });
        cx.run_until_parked();
        for _ in 0..4 {
            beat(cx, w, &view);
        }
    };
    let has = |cx: &mut TestAppContext, sel: &'static str| VisualTestContext::from_window(w, cx).debug_bounds(sel).is_some();
    send(cx, LlmEvent::ToolStarted { tool_id: "ui-run-0".into() });
    assert!(has(cx, "foot-Run") && !has(cx, "foot-Stream"), "a call running, no output: Run");
    send(cx, LlmEvent::ToolResultDelta { tool_id: "ui-run-0".into(), delta: "running 3 tests\n".into() });
    assert!(has(cx, "foot-Stream"), "its output coming in: Stream");
    send(cx, LlmEvent::ToolResult { tool_id: "ui-run-0".into(), result: "ok".into() });
    assert!(has(cx, "foot-Run") && !has(cx, "foot-Stream"), "the call done, the turn running: Run");
    send(cx, LlmEvent::Text { text: "Here is what changed: ".into() });
    assert!(has(cx, "foot-Stream"), "prose being written: Stream");
}

/// Finish gate #6 (live-full-allow f2841): a call drawn running never
/// steps back to "Queued" when its queued status comes again.
#[gpui_kit::test]
fn a_started_call_never_reads_queued_again(cx: &mut TestAppContext) {
    use synaps_client::protocol::{LlmEvent, StreamEvent};
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    view.update(cx, |v, _| v.client = None);
    let id = || "ui-run-0".to_string();
    let apply = |cx: &mut TestAppContext, evs: Vec<LlmEvent>| {
        view.update(cx, |v, cx| {
            for e in evs {
                v.model.transcript.apply(&SessionEvent::Stream { event: StreamEvent::Llm(e) });
            }
            v.sync_list();
            v.list.scroll_to_end();
            cx.notify();
        });
        cx.run_until_parked();
        fluid_frame(cx, w, &view);
    };
    let has = |cx: &mut TestAppContext, sel: &'static str| VisualTestContext::from_window(w, cx).debug_bounds(sel).is_some();
    apply(
        cx,
        vec![
            LlmEvent::ResponseStart,
            LlmEvent::ToolUseStart { tool_name: "bash".into(), tool_id: id() },
            LlmEvent::ToolUse { tool_name: "bash".into(), tool_id: id(), input: serde_json::json!({"command": "cargo build"}) },
            LlmEvent::ToolQueued { tool_id: id() },
        ],
    );
    assert!(has(cx, "life-Queued"), "queued before it starts");
    apply(cx, vec![LlmEvent::ToolStarted { tool_id: id() }]);
    wait(cx, w, &view, 400);
    assert!(has(cx, "life-Run") && !has(cx, "life-Queued"), "running");
    apply(cx, vec![LlmEvent::ToolQueued { tool_id: id() }]);
    assert!(has(cx, "life-Run") && !has(cx, "life-Queued"), "never Queued again (no turn either)");
    wait(cx, w, &view, 400);
    assert!(has(cx, "life-Run") && !has(cx, "life-Queued"), "never Queued again");
}

/// Shady #17 M1 (motion.md §5.3): the link lost mid-turn, the window is
/// visibly stale and nothing in it is live. Past the grace and the drain,
/// no orbit and no spinner is on the layer, and the one timer left is the
/// connection words' dots (2 Hz): never the orbit's 30 Hz, never the beat.
#[gpui_kit::test]
fn a_lost_link_stills_every_ambient(cx: &mut TestAppContext) {
    use crate::model::ConnState;
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    view.update(cx, |v, _| v.client = None);
    open_idle_cpu(cx, &view, crate::tabs::Place::Current);
    running_tools(cx, &view, 1);
    view.update(cx, |v, cx| {
        v.model.transcript.set_busy(true);
        cx.notify();
    });
    let conn = |cx: &mut TestAppContext, c: ConnState| {
        view.update(cx, |v, cx| {
            v.synthetic = false;
            v.model.conn = c;
            cx.notify();
        });
        cx.run_until_parked();
    };
    conn(cx, ConnState::Connected("x".into()));
    for _ in 0..6 {
        beat(cx, w, &view);
    }
    let ambient = |cx: &mut TestAppContext| {
        view.read_with(cx, |v, _| {
            let sites = v.fx.sites();
            (
                sites.iter().filter(|s| matches!(s.beat, crate::motion::Beat::Lattice(_))).count(),
                sites.iter().filter(|s| matches!(s.beat, crate::motion::Beat::Spin(_))).count(),
            )
        })
    };
    let (orbits, spins) = ambient(cx);
    assert!(orbits >= 1 && spins >= 1, "running: the orbits lap, the spinners spin ({orbits}, {spins})");
    conn(cx, ConnState::Reconnecting);
    wait_past(cx, w, &view, (crate::motion::LINK_GRACE + crate::motion::SLOW).as_millis() as u64);
    wait(cx, w, &view, 600);
    assert!(view.read_with(cx, |v, _| v.link.lost), "lost");
    assert_eq!(ambient(cx), (0, 0), "held: no orbit, no spinner");
    for _ in 0..8 {
        beat(cx, w, &view);
    }
    assert_eq!(ambient(cx), (0, 0), "still held");
    let (frame, at) = motion_pending(cx, &view);
    assert!(!frame, "no display frames while held");
    let at = at.expect("the dots step");
    assert!(view.read_with(cx, |v, _| v.fx.on_beats(at, 4)), "only the dots' 2 Hz step, not the beat or 30 Hz");
    assert_eq!(view.read_with(cx, |v, _| v.fx.wakes()), Some(crate::motion::Wake::App), "the words' frame, no layer");
}

/// Shady #17 R2-L1 (motion.md §5.3): the link lost, a running worker's
/// clocks hold their count as of the loss, frame after frame, while the
/// connection words' dots step; back, they count again.
#[gpui_kit::test]
fn a_lost_link_holds_every_clock(cx: &mut TestAppContext) {
    use crate::model::ConnState;
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    life_worker(cx, w, &view);
    let conn = |cx: &mut TestAppContext, c: ConnState| {
        view.update(cx, |v, cx| {
            v.synthetic = false;
            v.model.conn = c;
            cx.notify();
        });
        cx.run_until_parked();
    };
    conn(cx, ConnState::Connected("x".into()));
    wait(cx, w, &view, 500);
    // What the clocks show as of the last frame (the tray's and the chips'
    // text, which the frame's state decides).
    let shown = |cx: &mut TestAppContext| {
        view.read_with(cx, |v, _| {
            v.tray
                .rows
                .iter()
                .filter(|r| r.tick.is_some())
                .map(|r| crate::tray::ticked(&v.fx, &r.facts, &r.tick).to_string())
                .collect::<Vec<_>>()
        })
    };
    conn(cx, ConnState::Reconnecting);
    wait_past(cx, w, &view, (crate::motion::LINK_GRACE + crate::motion::SLOW).as_millis() as u64);
    wait(cx, w, &view, 600);
    assert!(view.read_with(cx, |v, _| v.link.lost), "lost");
    let held = shown(cx);
    assert!(!held.is_empty(), "a running worker's clock");
    let f0 = frames(cx, &view).0;
    for _ in 0..30 {
        wait(cx, w, &view, 100);
        assert_eq!(shown(cx), held, "held through the outage");
    }
    assert!(frames(cx, &view).0 > f0, "the dots stepped (the app drew) meanwhile");
    conn(cx, ConnState::Connected("x".into()));
    for _ in 0..15 {
        wait(cx, w, &view, 100);
    }
    assert_ne!(shown(cx), held, "back: the clocks count again");
}

/// [`running_without_a_clock`] without the layer: the sites stay empty.
fn running_without_a_clock_in_frame(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    view.update(cx, |v, _| v.client = None);
    open_idle_cpu(cx, view, crate::tabs::Place::Current);
    set_motion(cx, view, crate::motion::Level::Full);
    running_tools(cx, view, 1);
    for _ in 0..12 {
        beat(cx, w, view);
    }
}

/// The app's view is cached: after beats that repainted only the layer,
/// every real change still draws it: an event, a hover, a scroll, a
/// resize, a focus change, a theme change, the motion level.
#[gpui_kit::test]
fn a_change_after_beats_still_draws(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    for _ in 0..12 {
        beat(cx, w, &view);
    }
    let drew = |cx: &mut TestAppContext, what: &str, change: &mut dyn FnMut(&mut TestAppContext)| {
        for _ in 0..3 {
            beat(cx, w, &view);
        }
        let f0 = frames(cx, &view).0;
        change(cx);
        cx.run_until_parked();
        settle(cx, w, 4);
        assert!(frames(cx, &view).0 > f0, "{what}: the app's view was not drawn");
    };
    // The rail's spinners and the cards' lattices (indicators/proposal.md §8).
    let spins = |cx: &mut TestAppContext| {
        view.read_with(cx, |v, _| {
            v.fx.sites().iter().filter(|s| matches!(s.beat, crate::motion::Beat::Spin(_) | crate::motion::Beat::Lattice(_))).count()
        })
    };
    let sites = spins(cx);
    // As the pump hands it over (Shady N3): `apply_batch` notifies, not the test.
    drew(cx, "an event", &mut |cx| {
        use synaps_client::protocol::{LlmEvent, StreamEvent};
        view.update(cx, |v, cx| {
            let a = v.model.active.as_ref().expect("attached");
            let (attach_seq, session_id) = (a.attach_seq, a.session_id.clone());
            let ev = |seq: u64, event: StreamEvent| {
                synaps_client::Event::session(
                    attach_seq,
                    session_id.clone(),
                    seq,
                    SessionEvent::Stream { event },
                    std::sync::Arc::new(serde_json::Value::Null),
                )
            };
            let id = "ui-batch-0".to_string();
            let start = LlmEvent::ToolUseStart { tool_name: "bash".into(), tool_id: id.clone() };
            let input = serde_json::json!({"command": "cargo test -p relay"});
            v.list.scroll_to_end();
            v.apply_batch(
                vec![
                    ev(900, StreamEvent::Llm(LlmEvent::ResponseStart)),
                    ev(901, StreamEvent::Llm(start)),
                    ev(902, StreamEvent::Llm(LlmEvent::ToolUse { tool_name: "bash".into(), tool_id: id, input })),
                ],
                cx,
            );
        });
    });
    assert!(spins(cx) > sites, "the new call's glyph is on the layer");
    let halt = VisualTestContext::from_window(w, cx).debug_bounds("halt").expect("HALT");
    drew(cx, "a hover", &mut |cx| {
        event(
            cx,
            w,
            PlatformInput::MouseMove(gpui_kit::MouseMoveEvent {
                position: halt.center(),
                pressed_button: None,
                modifiers: Default::default(),
            }),
        )
    });
    let top = view.read_with(cx, |v, _| v.list.logical_scroll_top());
    drew(cx, "a scroll", &mut |cx| {
        event(
            cx,
            w,
            PlatformInput::Scroll(gpui_kit::ScrollEvent {
                position: gpui_kit::point(px(720.), px(300.)),
                delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(240.))),
                ..Default::default()
            }),
        )
    });
    assert_ne!(view.read_with(cx, |v, _| v.list.logical_scroll_top()), top, "it scrolled");
    drew(cx, "a resize", &mut |cx| {
        VisualTestContext::from_window(w, cx).simulate_resize(size(px(1300.), px(900.)));
    });
    assert_eq!(view.read_with(cx, |v, _| v.width), 1300.);
    drew(cx, "a focus change", &mut |cx| {
        cx.update_window(w, |_, window, cx| {
            let h = view.read(cx).composer.focus_handle(cx);
            if h.is_focused(window) { window.blur(cx) } else { window.focus(&h, cx) }
        })
        .expect("the window");
    });
    drew(cx, "a theme change (a global)", &mut |cx| cx.update(|cx| crate::theme::update(cx, |c| c.accent = Some(0x00ff_8800))));
    drew(cx, "the motion level (a global)", &mut |cx| {
        cx.update(|cx| {
            cx.set_global(crate::motion::Setting { level: Some(crate::motion::Level::Reduced), lock: None });
            crate::motion::Level::apply(cx);
        })
    });
    assert!(view.read_with(cx, |v, _| v.fx.sites().is_empty()), "Reduced: no beat, nothing on the layer");
}

// ── fluid (cohesion/fluid.md §7, F1) ──────────────────────────────────

/// One display frame (144 Hz) later: the motion clock (the test's, [`open_with`])
/// and the executor's move on together, then the app draws.
fn fluid_frame(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    let dt = std::time::Duration::from_micros(6944);
    view.read_with(cx, |v, _| v.fx.set_clock(v.fx.clock() + dt));
    cx.executor().advance_clock(dt);
    view.update(cx, |_, cx| cx.notify());
    cx.update_window(w, |_, window, cx| window.simulate_next_frame(cx)).expect("the window");
    cx.run_until_parked();
}

/// Shady #23 r2 H1: the list's end is its bottom, exactly, whatever frames
/// land mid-growth. Each frame rate puts different rows out of view in the
/// middle of their growth, where the list keeps the height it last
/// measured; the rows never drawn keep a hint, the mean of the rows above
/// (a ninth: `.2222`). A fractional one put CI's end an f32 ulp off. At 24
/// to 240 Hz: the end is the bottom every frame, every row drawn is whole,
/// and so is the list's extent (the hints too).
#[gpui_kit::test]
fn the_lists_end_is_exact_at_any_frame_rate(cx: &mut TestAppContext) {
    for hz in [24u64, 37, 60, 144, 240] {
        let (w, view, _events) = open(cx);
        set_motion(cx, &view, crate::motion::Level::Full);
        view.update(cx, |v, _| v.client = None);
        start_stream(cx, &view);
        settle(cx, w, 8);
        let (extent, vh) =
            view.read_with(cx, |v, _| (f32::from(v.list.max_offset_for_scrollbar().y), f32::from(v.list.viewport_bounds().size.height)));
        assert_eq!((extent + vh).fract(), 0., "{hz} Hz: opened, the rows' sum ({extent} past {vh}; the rows above, hinted)");
        view.update(cx, |v, _| v.list.scroll_to_end());
        let dt = std::time::Duration::from_micros(1_000_000 / hz);
        for k in 0..90 {
            stream_step(cx, &view, k);
            view.read_with(cx, |v, _| v.fx.set_clock(v.fx.clock() + dt));
            cx.executor().advance_clock(dt);
            view.update(cx, |_, cx| cx.notify());
            cx.update_window(w, |_, window, cx| window.simulate_next_frame(cx)).expect("the window");
            cx.run_until_parked();
            let (gap, pad, frac) = view.read_with(cx, |v, cx| {
                let n = v.list.item_count();
                let pad = (28. * crate::theme::metrics(cx).r).round();
                let gap = v.list.bounds_for_item(n - 1).map(|b| f32::from(v.list.viewport_bounds().bottom() - b.bottom()));
                let frac: Vec<(usize, f32)> = (0..n)
                    .filter_map(|ix| v.list.bounds_for_item(ix).map(|b| (ix, f32::from(b.size.height))))
                    .filter(|r| r.1.fract() != 0.)
                    .collect();
                (gap, pad, frac)
            });
            assert!(frac.is_empty(), "{hz} Hz, frame {k}: rows off the pixel grid {frac:?}");
            assert_eq!(gap, Some(pad), "{hz} Hz, frame {k}: the end is the bottom");
            // The rows not drawn too (their hints): the rows' sum is whole
            // (the extent past a viewport that a dock's spring may hold
            // mid-pixel).
            let (extent, vh) = view
                .read_with(cx, |v, _| (f32::from(v.list.max_offset_for_scrollbar().y), f32::from(v.list.viewport_bounds().size.height)));
            assert_eq!((extent + vh).fract(), 0., "{hz} Hz, frame {k}: the rows' sum ({extent} past {vh})");
        }
    }
}

/// A turn starts streaming in the shown session.
fn start_stream(cx: &mut TestAppContext, view: &Entity<AppView>) {
    use synaps_client::protocol::{LlmEvent, StreamEvent};
    view.update(cx, |v, cx| {
        let t = &mut v.model.transcript;
        let n = t.items().len();
        t.apply(&SessionEvent::TurnStarted {
            turn_baseline: n as _,
            trigger: "user".into(),
            user_text: Some("Walk me through it.".into()),
        });
        t.apply(&SessionEvent::Stream { event: StreamEvent::Llm(LlmEvent::ResponseStart) });
        t.set_busy(true);
        cx.notify();
    });
    cx.run_until_parked();
}

/// Frame `k` of a stream: a few words (a line wraps every few frames), and
/// every 25th frame a tool row arrives.
fn stream_step(cx: &mut TestAppContext, view: &Entity<AppView>, k: usize) {
    use synaps_client::protocol::{LlmEvent, StreamEvent};
    view.update(cx, |v, cx| {
        let t = &mut v.model.transcript;
        if k % 25 == 12 {
            let id = format!("fluid-{k}");
            t.apply(&SessionEvent::Stream {
                event: StreamEvent::Llm(LlmEvent::ToolUseStart { tool_name: "bash".into(), tool_id: id.clone() }),
            });
            t.apply(&SessionEvent::Stream {
                event: StreamEvent::Llm(LlmEvent::ToolUse {
                    tool_name: "bash".into(),
                    tool_id: id,
                    input: serde_json::json!({"command": "cargo test"}),
                }),
            });
        }
        t.apply(&SessionEvent::Stream {
            event: StreamEvent::Llm(LlmEvent::Text { text: format!("word{k} and some more text to wrap, ") }),
        });
        cx.notify();
    });
}

/// Shady #23 round 2 H1 (CI run 44363749232): every transcript row rests
/// on whole pixels, at every text size and at the tight width. The demo's
/// authorize card was 157.5 px (its 15 px title's 1.5 line height): a half
/// pixel in the list's sum put `follow_offset_is_end_every_frame`'s end
/// an f32 ulp off its bottom on CI's runners (28.000061 for 28), while
/// bella's content happened to sum exactly.
#[gpui_kit::test]
fn every_row_rests_on_whole_pixels(cx: &mut TestAppContext) {
    for text in [13u8, 14, 15, 16, 17, 18, 20] {
        for width in [1440f32, 1100.] {
            let (w, view, _events) = open_at(cx, (width, 9000.), text);
            set_motion(cx, &view, crate::motion::Level::Full);
            view.update(cx, |v, _| v.client = None);
            settle(cx, w, 8);
            let (n, frac) = view.read_with(cx, |v, _| {
                let n = v.list.item_count();
                let frac: Vec<_> = (0..n)
                    .filter_map(|ix| v.list.bounds_for_item(ix).map(|b| (ix, v.disp.get(ix).map(|d| d.kind), f32::from(b.size.height))))
                    .filter(|r| r.2.fract() != 0.)
                    .collect();
                (n, frac)
            });
            assert!(n > 5, "the demo's rows");
            assert!(frac.is_empty(), "text {text} at {width}: rows off the pixel grid {frac:?}");
            let card = VisualTestContext::from_window(w, cx).debug_bounds("card").expect("the demo's card");
            assert_eq!(f32::from(card.size.height).fract(), 0., "text {text} at {width}: the card {card:?}");
            // A turn running: the card says so in its footer (the state
            // `follow_offset_is_end_every_frame` starts from).
            start_stream(cx, &view);
            settle(cx, w, 8);
            assert!(VisualTestContext::from_window(w, cx).debug_bounds("card-footer").is_some(), "text {text}: the card's footer");
            let frac = view.read_with(cx, |v, _| {
                (0..v.list.item_count())
                    .filter_map(|ix| v.list.bounds_for_item(ix).map(|b| (ix, v.disp.get(ix).map(|d| d.kind), f32::from(b.size.height))))
                    .filter(|r| r.2.fract() != 0.)
                    .collect::<Vec<_>>()
            });
            assert!(frac.is_empty(), "text {text} at {width}, a turn running: rows off the pixel grid {frac:?}");
        }
    }
}

/// Invariant (a): while following, the end of content is the bottom of the
/// view on every frame, as text streams into the last row (its new lines
/// gliding in) and rows arrive (revealing their height).
#[gpui_kit::test]
fn follow_offset_is_end_every_frame(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    view.update(cx, |v, _| v.client = None);
    start_stream(cx, &view);
    settle(cx, w, 8);
    view.update(cx, |v, _| v.list.scroll_to_end());
    fluid_frame(cx, w, &view);
    let mut grew = 0;
    for k in 0..90 {
        stream_step(cx, &view, k);
        fluid_frame(cx, w, &view);
        let (following, gap, pad, growing) = view.read_with(cx, |v, cx| {
            let n = v.list.item_count();
            let pad = (28. * crate::theme::metrics(cx).r).round();
            let bottom = v.list.bounds_for_item(n - 1).map(|b| f32::from(v.list.viewport_bounds().bottom() - b.bottom()));
            (v.list.is_following_tail(), bottom, pad, v.fx.growing())
        });
        assert!(following, "frame {k}: following");
        assert_eq!(gap, Some(pad), "frame {k}: the end is the bottom");
        grew = grew.max(growing);
    }
    assert!(grew > 0, "rows revealed and grew while following");
}

/// Invariant (b): scrolled back, a row on screen never moves, not by a
/// pixel, while text streams in and rows arrive below it.
#[gpui_kit::test]
fn anchor_row_never_moves(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    view.update(cx, |v, _| v.client = None);
    start_stream(cx, &view);
    settle(cx, w, 8);
    for k in 0..30 {
        stream_step(cx, &view, k);
        fluid_frame(cx, w, &view);
    }
    view.update(cx, |v, _| v.list.scroll_by(gpui_kit::px(-260.)));
    fluid_frame(cx, w, &view);
    let (anchor, top) = view.read_with(cx, |v, _| {
        let ix = v.list.logical_scroll_top().item_ix + 1;
        (ix, v.list.bounds_for_item(ix).map(|b| b.top()))
    });
    assert!(top.is_some(), "the anchor row is on screen");
    for k in 30..120 {
        stream_step(cx, &view, k);
        fluid_frame(cx, w, &view);
        let (following, now) = view.read_with(cx, |v, _| (v.list.is_following_tail(), v.list.bounds_for_item(anchor).map(|b| b.top())));
        assert!(!following, "frame {k}");
        assert_eq!(now, top, "frame {k}: the anchor row moved");
    }
}

/// §3.4: the streaming row's height glides to each new line, and is never
/// more than a line short of its text.
#[gpui_kit::test]
fn stream_reveal_never_trails_a_line(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    // The demo's prompt answered first: its card's row would follow the
    // streaming row (polish.md §2.6), and this is about the last row.
    resolve_prompt(cx, &view);
    set_motion(cx, &view, crate::motion::Level::Full);
    view.update(cx, |v, _| v.client = None);
    start_stream(cx, &view);
    settle(cx, w, 8);
    let (mut glided, mut seen) = (0, 0);
    for k in 0..100 {
        stream_step(cx, &view, k * 25 + 1);
        fluid_frame(cx, w, &view);
        let (state, line) = view.read_with(cx, |v, cx| {
            let id = v.disp.last().map(|d| d.id).unwrap_or(0);
            (v.fx.grow_of(id), crate::theme::metrics(cx).body_line)
        });
        // Its arrival reveals all of it on `snap` first (Shady N7); then its
        // new lines glide.
        let arriving = view.read_with(cx, |v, _| v.disp.last().is_some_and(|d| v.fx.arriving(d.id)));
        if let Some((shown, natural)) = state.filter(|(s, n)| s.is_finite() && n.is_finite() && !arriving) {
            seen += 1;
            assert!(shown >= natural - line - 1e-3, "frame {k}: {shown} trails {natural} by more than a line ({line})");
            if shown < natural {
                glided += 1;
            }
        }
    }
    assert!(seen > 40, "the streaming row was tracked ({seen} frames)");
    assert!(glided > 0, "its new lines glided in");
}

/// Reduced is the build before F1: the list's own wheel (one notch, 60 px,
/// in the frame it lands), rows that appear in one frame, no layer.
#[gpui_kit::test]
fn reduced_matches_today(cx: &mut TestAppContext) {
    use gpui_kit::{Modifiers, MouseMoveEvent, ScrollDelta, ScrollEvent, ScrollSource, TouchPhase, point};
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Reduced);
    view.update(cx, |v, _| v.client = None);
    start_stream(cx, &view);
    settle(cx, w, 8);
    for k in 0..30 {
        stream_step(cx, &view, k);
        fluid_frame(cx, w, &view);
    }
    view.update(cx, |v, _| v.list.scroll_by(gpui_kit::px(-400.)));
    frame(cx, &view);
    settle(cx, w, 8);
    let center = view.read_with(cx, |v, _| v.list.viewport_bounds().center());
    let offset = |cx: &mut TestAppContext| view.read_with(cx, |v, _| -f32::from(v.list.scroll_px_offset_for_scrollbar().y));
    let before = offset(cx);
    event(cx, w, PlatformInput::MouseMove(MouseMoveEvent { position: center, pressed_button: None, modifiers: Modifiers::default() }));
    event(
        cx,
        w,
        PlatformInput::Scroll(ScrollEvent {
            position: center,
            delta: ScrollDelta::Lines(point(0., -3.)),
            modifiers: Modifiers::default(),
            touch_phase: TouchPhase::Moved,
            source: ScrollSource::Pointer,
        }),
    );
    settle(cx, w, 2);
    assert_eq!(offset(cx) - before, 60., "the list's own wheel: a notch at once");
    settle(cx, w, 4);
    assert_eq!(offset(cx) - before, 60., "and nothing after it");
    assert!(!view.read_with(cx, |v, _| v.scroller.active()));
    for k in 0..40 {
        stream_step(cx, &view, k);
        fluid_frame(cx, w, &view);
        assert_eq!(view.read_with(cx, |v, _| (v.fx.growing(), v.layer.is_some())), (0, false), "frame {k}");
    }
    // Full: the same notch is ours, and glides.
    set_motion(cx, &view, crate::motion::Level::Full);
    let before = offset(cx);
    event(
        cx,
        w,
        PlatformInput::Scroll(ScrollEvent {
            position: center,
            delta: ScrollDelta::Lines(point(0., 3.)),
            modifiers: Modifiers::default(),
            touch_phase: TouchPhase::Moved,
            source: ScrollSource::Pointer,
        }),
    );
    assert!(view.read_with(cx, |v, _| v.scroller.active()), "Full: the scroller took it");
    fluid_frame(cx, w, &view);
    let first = offset(cx) - before;
    assert!(first < 0. && first > -60., "glides: {first}");
}

/// History never animates: a session attached (its rows a history load)
/// draws every row at its height on its first frame; only the column
/// arrives, as one layer.
#[gpui_kit::test]
fn history_never_animates(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    view.update(cx, |v, _| v.client = None);
    settle(cx, w, 8);
    // Not a rapid switch (within 150 ms of the launch's column).
    view.read_with(cx, |v, _| v.fx.set_clock(v.fx.clock() + std::time::Duration::from_millis(300)));
    open_idle_cpu(cx, &view, crate::tabs::Place::Current);
    assert!(view.read_with(cx, |v, _| v.layer.is_some()), "the column arrives as one layer");
    for k in 0..60 {
        fluid_frame(cx, w, &view);
        assert_eq!(view.read_with(cx, |v, _| v.fx.growing()), 0, "frame {k}: no history row grows");
    }
    assert!(view.read_with(cx, |v, _| v.layer.is_none()), "the layer settled");
}

/// Shady H1, finish gate fix 2: a row that reveals or streams clips its
/// body, not its gutter. Every frame the last prose row streams (wrapped),
/// the write head is drawn on its shown bottom, left of the row, and its
/// `AGENT` label is beside it.
#[gpui_kit::test]
fn the_gutter_stays_drawn_while_a_row_grows(cx: &mut TestAppContext) {
    use gpui_kit::VisualTestContext;
    // Wide enough that the labels hang in the gutter.
    let (w, view, _events) = open_at(cx, (2200., 1000.), crate::theme::Panels::default().text_size);
    assert!(view.read_with(cx, |v, _| v.layout.hang), "the labels hang");
    // The demo's prompt answered first: its card's row would follow the
    // streaming row (polish.md §2.6), and this is about the last row.
    resolve_prompt(cx, &view);
    set_motion(cx, &view, crate::motion::Level::Full);
    view.update(cx, |v, _| v.client = None);
    start_stream(cx, &view);
    settle(cx, w, 8);
    view.update(cx, |v, _| v.list.scroll_to_end());
    fluid_frame(cx, w, &view);
    let mut seen = 0;
    for k in 0..40 {
        stream_step(cx, &view, k * 25 + 1);
        fluid_frame(cx, w, &view);
        let id = view.read_with(cx, |v, _| v.disp.last().map(|d| d.id).filter(|id| v.fx.grow_of(*id).is_some()));
        let Some(id) = id else { continue };
        let mut vcx = VisualTestContext::from_window(w, cx);
        let clip = vcx.debug_bounds(String::leak(format!("grow-clip-{id}"))).expect("the streaming row is wrapped");
        let head = vcx.debug_bounds("write-head").unwrap_or_else(|| panic!("frame {k}: the write head is drawn"));
        let label = vcx.debug_bounds(String::leak(format!("hang-label-{id}"))).unwrap_or_else(|| panic!("frame {k}: AGENT is drawn"));
        assert!((head.bottom() - clip.bottom()).abs() < px(0.5), "frame {k}: the head {head:?} sits on the shown bottom of {clip:?}");
        assert!(head.right() <= clip.left() && label.right() <= clip.left(), "frame {k}: both in the gutter");
        seen += 1;
    }
    assert!(seen > 20, "the streaming row was wrapped ({seen} frames)");
}

/// Shady M1: rows that arrive below a view scrolled back by less than the
/// list's overdraw are laid out off screen, never painted: they don't
/// reveal (the 0 px an arrival starts at would stay in GPUI's heights).
/// Within two frames the list's end is where their real heights put it.
#[gpui_kit::test]
fn arrivals_off_screen_keep_their_height(cx: &mut TestAppContext) {
    use synaps_client::protocol::{LlmEvent, StreamEvent};
    let (w, view, _events) = open(cx);
    // The demo's prompt answered first, its card's collapse landed: its row
    // would follow the streaming row (polish.md §2.6), and this measures
    // the rows that arrive.
    resolve_prompt(cx, &view);
    wait(cx, w, &view, 1000);
    set_motion(cx, &view, crate::motion::Level::Full);
    view.update(cx, |v, _| v.client = None);
    start_stream(cx, &view);
    settle(cx, w, 8);
    for k in 0..30 {
        stream_step(cx, &view, k * 25 + 1);
        fluid_frame(cx, w, &view);
    }
    for _ in 0..40 {
        fluid_frame(cx, w, &view);
    }
    view.update(cx, |v, _| v.list.scroll_by(px(-200.)));
    fluid_frame(cx, w, &view);
    let max = |cx: &mut TestAppContext| view.read_with(cx, |v, _| f32::from(v.list.max_offset_for_scrollbar().y));
    let (max0, n0) = (max(cx), view.read_with(cx, |v, _| v.list.item_count()));
    view.update(cx, |v, cx| {
        let t = &mut v.model.transcript;
        for k in 0..3 {
            let id = format!("off-{k}");
            t.apply(&SessionEvent::Stream {
                event: StreamEvent::Llm(LlmEvent::ToolUseStart { tool_name: "bash".into(), tool_id: id.clone() }),
            });
            t.apply(&SessionEvent::Stream {
                event: StreamEvent::Llm(LlmEvent::ToolUse {
                    tool_name: "bash".into(),
                    tool_id: id,
                    input: serde_json::json!({"command": "ls"}),
                }),
            });
        }
        cx.notify();
    });
    fluid_frame(cx, w, &view);
    fluid_frame(cx, w, &view);
    let (max1, n1) = (max(cx), view.read_with(cx, |v, _| v.list.item_count()));
    assert!(n1 > n0, "rows arrived");
    assert!(view.read_with(cx, |v, _| !v.list.is_following_tail()), "still scrolled back");
    // Their real heights: at the end, at rest.
    view.update(cx, |v, _| v.list.scroll_to_end());
    for _ in 0..60 {
        fluid_frame(cx, w, &view);
    }
    let real: f32 = view.read_with(cx, |v, _| (n0..n1).filter_map(|ix| v.list.bounds_for_item(ix)).map(|b| f32::from(b.size.height)).sum());
    assert!(real > 20., "the rows have height ({real})");
    assert!((max1 - max0 - real).abs() < 1., "the end grew by {} for rows {real} tall", max1 - max0);
}

/// Finish gate fix 1, Shady M3 and L7: the column's layer starts on the
/// first frame that draws the column (a tab's kept rows, its attach still
/// on its way), and the attach replays nothing; it is keyed by the tab's
/// id, so closing another tab moves nothing; a switch straight back
/// (within 150 ms) shows no slide; Settings, none; back to the session from
/// a worker's view, from above.
#[gpui_kit::test]
fn the_column_layer_starts_with_the_column(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    view.read_with(cx, |v, _| v.fx.pin_clock());
    // `ms` on from the last frame's time (a timer the executor ran to may
    // have put that past the clock): frames this far apart, at least.
    let wait = |cx: &mut TestAppContext, ms: u64| {
        let d = std::time::Duration::from_millis(ms);
        view.read_with(cx, |v, _| v.fx.set_clock(v.fx.tick() + d));
        cx.executor().advance_clock(d);
        settle(cx, w, 8);
    };
    let layer = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.layer);
    let focus = |cx: &mut TestAppContext, ix: usize| {
        view.update(cx, |v, cx| {
            v.focus_tab(ix);
            cx.notify();
        });
        cx.run_until_parked();
    };
    wait(cx, 400);
    // A worker's view in place: from below; back to the session: from above.
    scout_in_place(cx, w, &view);
    assert_eq!(layer(cx).map(|l| (l.1, l.2)), Some((0., 1.)), "a worker's view, from below");
    wait(cx, 400);
    view.update(cx, |v, cx| v.leave_worker(false, cx));
    cx.run_until_parked();
    assert_eq!(layer(cx).map(|l| (l.1, l.2)), Some((0., -1.)), "back to the session, from above");
    wait(cx, 400);
    // A new tab right of this one: from the right.
    let client = view.update(cx, |v, _| v.client.take());
    open_idle_cpu(cx, &view, crate::tabs::Place::New);
    assert_eq!(layer(cx).map(|l| (l.1, l.2)), Some((1., 0.)), "a tab to the right, from the right");
    wait(cx, 400);
    // Back to the first tab, its attach on its way: its kept rows draw, and
    // the layer starts with them.
    let tx = view.update(cx, |v, _| v.synthetic_tx.take());
    focus(cx, 0);
    let (attached_now, rows) = view.read_with(cx, |v, _| (v.model.active.is_some(), v.disp.len()));
    assert!(!attached_now && rows > 0, "its kept rows on screen, not attached yet");
    let (at, x, y) = layer(cx).expect("the layer starts on the frame that draws the column");
    assert_eq!((x, y), (-1., 0.), "a tab to the left, from the left");
    wait(cx, 20);
    view.update(cx, |v, cx| {
        v.synthetic_tx = tx;
        v.synthetic_attach(crate::demo::SID, None, false);
        cx.notify();
    });
    cx.run_until_parked();
    assert_eq!(attached(cx, &view).as_deref(), Some(crate::demo::SID), "attached");
    assert_eq!(layer(cx).map(|l| l.0), Some(at), "the attach replays nothing");
    wait(cx, 400);
    // A switch, and straight back within 150 ms: no slide. (Drawn frame by
    // frame: nothing runs the executor on to a timer between the two.)
    let draw = |cx: &mut TestAppContext, ix: usize, ms: u64| {
        view.update(cx, |v, cx| {
            v.fx.set_clock(v.fx.tick() + std::time::Duration::from_millis(ms));
            v.focus_tab(ix);
            cx.notify();
        });
        cx.update_window(w, |_, window, cx| window.simulate_next_frame(cx)).expect("the window");
    };
    draw(cx, 1, 0);
    assert!(layer(cx).is_some(), "a switch: a layer");
    draw(cx, 0, 60);
    assert_eq!(layer(cx), None, "a switch straight back shows no slide");
    cx.run_until_parked();
    wait(cx, 400);
    // Closing the tab left of the one on screen moves nothing.
    focus(cx, 1);
    wait(cx, 400);
    view.update(cx, |v, cx| {
        v.close_tab(0);
        cx.notify();
    });
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |v, _| v.model.tabs.active()), Some(0));
    assert_eq!(layer(cx), None, "the same column, at another place: no layer");
    // Settings: its own pane, no layer.
    view.update(cx, |v, cx| v.open_view(crate::tabs::Kind::Settings, cx));
    cx.run_until_parked();
    assert_eq!(layer(cx), None, "Settings: no layer");
    view.update(cx, |v, _| v.client = client);
}

/// Shady M2: every list swap goes through `list_out`, which stops the
/// scroller: a glide in flight never carries onto the next list (a worker's
/// view, a session not in the tab cache).
#[gpui_kit::test]
fn a_list_swap_stops_the_scroller(cx: &mut TestAppContext) {
    use gpui_kit::{Modifiers, MouseMoveEvent, ScrollDelta, ScrollEvent, ScrollSource, TouchPhase, point};
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    settle(cx, w, 8);
    let over = point(px(720.), px(300.));
    event(cx, w, PlatformInput::MouseMove(MouseMoveEvent { position: over, pressed_button: None, modifiers: Modifiers::default() }));
    let notch = |cx: &mut TestAppContext| {
        event(
            cx,
            w,
            PlatformInput::Scroll(ScrollEvent {
                position: over,
                delta: ScrollDelta::Lines(point(0., 3.)),
                modifiers: Modifiers::default(),
                touch_phase: TouchPhase::Moved,
                source: ScrollSource::Pointer,
            }),
        )
    };
    notch(cx);
    assert!(view.read_with(cx, |v, _| v.scroller.active()), "a glide in flight");
    scout_in_place(cx, w, &view);
    assert!(view.read_with(cx, |v, _| !v.scroller.active()), "a worker's view: stopped");
    view.update(cx, |v, cx| v.leave_worker(false, cx));
    cx.run_until_parked();
    notch(cx);
    assert!(view.read_with(cx, |v, _| v.scroller.active()), "a glide in flight");
    view.update(cx, |v, _| v.client = None);
    open_idle_cpu(cx, &view, crate::tabs::Place::Current);
    assert!(view.read_with(cx, |v, _| !v.scroller.active()), "another session: stopped");
}

/// Message times (`crate::times`): the demo's sessions are dated (fixed
/// local times, the same on any machine), so YOU and AGENT each show their
/// row's `HH:MM`: under the label in the margin of a wide pane, at the end
/// of the label's line in a narrow one. History never animates: no row of
/// it is born. The parked session, resumed the next morning, has its one
/// day line, over that turn.
#[gpui_kit::test]
fn labels_show_their_message_times(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    view.update(cx, |v, cx| {
        v.client = None;
        v.panels.rail_collapsed = true;
        v.panels.right_collapsed = true;
        cx.notify();
    });
    cx.run_until_parked();
    let labelled = |cx: &mut VisualTestContext| {
        view.read_with(cx, |v, _| {
            let rows = v.disp.iter().filter(|d| d.who != crate::turns::Who::None);
            rows.map(|d| (d.id, v.rows[d.item].time.as_ref().map(|t| t.hhmm.to_string()))).collect::<Vec<_>>()
        })
    };
    let mut vcx = VisualTestContext::from_window(w, cx);
    let rows = labelled(&mut vcx);
    assert!(rows.len() >= 2 && rows.iter().all(|(_, t)| t.is_some()), "every label has its time: {rows:?}");
    assert_eq!(rows[0].1.as_deref(), Some("04:12"), "the first message, at the session's start");
    let unborn = view.read_with(&vcx, |v, _| v.disp.iter().all(|d| v.fx.birth(d.id).is_none()));
    assert!(unborn, "history rows are not drawn in");
    let (id, _) = *rows.last().unwrap();
    let (time, label) = (leak(format!("time {id}")), leak(format!("label {id}")));
    for (width, hang) in [(1440., true), (640., false)] {
        vcx.simulate_resize(size(px(width), px(900.)));
        vcx.run_until_parked();
        view.update(&mut vcx, |v, cx| {
            let ix = v.disp.iter().position(|d| d.id == id).expect("the labelled row");
            v.list.scroll_to(gpui_kit::ListOffset { item_ix: ix, offset_in_item: px(0.) });
            cx.notify();
        });
        vcx.run_until_parked();
        assert_eq!(view.read_with(&vcx, |v, _| v.layout.hang), hang, "{width} px");
        let (t, l) = (vcx.debug_bounds(time).expect("the time drawn"), vcx.debug_bounds(label).expect("the label drawn"));
        if hang {
            assert!((t.right() - l.right()).abs() < px(1.) && t.top() >= l.bottom() - px(1.), "under the label: {t:?} {l:?}");
        } else {
            let mid = |b: Bounds<gpui_kit::Pixels>| b.top() + b.size.height / 2.;
            assert!(t.left() > l.right() + px(200.) && (mid(t) - mid(l)).abs() < px(2.), "at the line's end: {t:?} {l:?}");
        }
    }
    open_idle_cpu(cx, &view, crate::tabs::Place::Current);
    let days = view.read_with(cx, |v, _| {
        let items = v.model.shown().items();
        let last_user = items.iter().rposition(|i| i.kind == synaps_client::transcript::ItemKind::User);
        let days: Vec<_> = v.disp.iter().filter(|d| d.kind == crate::turns::Kind::Day).map(|d| Some(d.item)).collect();
        (days, last_user)
    });
    assert_eq!(days.0, [days.1], "one day line, over the turn of the next morning");
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// polish.md P6 (finish gate #17): favouriting moves the row into
/// FAVOURITES (its Flip); its star hops, and its sparks leave, only once the
/// row has landed: never while it moves.
#[gpui_kit::test]
fn a_star_hops_once_its_row_has_landed(cx: &mut TestAppContext) {
    use crate::rail::Item;
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    settle(cx, w, 8);
    // A live session's row, favourited (Ctrl+D's way: the pointer is not on
    // the rail, which holds its order under it).
    let key = view.update(cx, |v, cx| {
        let rail = &v.rail.rail;
        let id = crate::demo::IDLE_CPU;
        let ix = v.model.sessions.iter().position(|s| s.id == id).expect("listed");
        let key = rail
            .items
            .iter()
            .find_map(|i| if let Item::Row(r) = i { (rail.rows[*r].live == Some(ix)).then_some(rail.rows[*r].key) } else { None })
            .expect("its row");
        v.toggle_fav_session(id);
        cx.notify();
        key
    });
    let (mut moving_frames, mut landed_at) = (0, None);
    for k in 0..80 {
        let (moving, landed) = view.read_with(cx, |v, _| {
            (v.rail_fx.offsets.get(&key).is_some_and(|d| *d != 0.), v.star_land.borrow().get(&key).and_then(|e| e.1))
        });
        if moving {
            moving_frames += 1;
            assert!(landed.is_none(), "frame {k}: the star hops while its row moves");
        }
        if landed.is_some() && landed_at.is_none() {
            landed_at = Some(k);
        }
        wait(cx, w, &view, 8);
    }
    let flips = view.read_with(cx, |v, _| format!("{:?} {:?}", v.rail_fx.flips, v.rail_fx.offsets));
    assert!(moving_frames > 0, "the row slid into FAVOURITES: {flips}");
    assert!(landed_at.is_some(), "then it landed, and the star hopped");
}

/// Favourites (desktop only): a HISTORY row's star is laid out before it
/// is drawn (the row's text never moves when it shows), a click on it
/// favourites the row without opening it (no resume, nothing sent), and
/// the row moves to FAVOURITES, out of HISTORY; the favourite key
/// (Ctrl+D) favourites the session on screen; its row's star unfavourites
/// it. A favourite's row draws its filled star always.
#[gpui_kit::test]
fn a_star_favourites_a_row_without_opening_it(cx: &mut TestAppContext) {
    use crate::rail::Item;
    let (w, view, _events) = open(cx);
    settle(cx, w, 8);
    // The first HISTORY row the rail shows (it lists the demo's history).
    let (key, id) = view.update(cx, |v, cx| {
        let rail = &v.rail.rail;
        let (ix, r) = rail
            .items
            .iter()
            .enumerate()
            .find_map(|(ix, i)| if let Item::Hist(r) = i { Some((ix, *r)) } else { None })
            .expect("a history row");
        let entry = crate::rail::saved_entry(&v.model, rail.rows[r].saved.expect("saved")).expect("listed");
        let out = (rail.rows[r].key, entry.id.clone());
        v.rail_list.scroll_to_reveal_item(ix);
        cx.notify();
        out
    });
    settle(cx, w, 4);
    let star: &'static str = Box::leak(format!("star {key}").into_boxed_str());
    let bounds = |cx: &mut TestAppContext, sel: &'static str| VisualTestContext::from_window(w, cx).debug_bounds(sel);
    let before = bounds(cx, star).expect("the star's slot is laid out at rest");
    event(
        cx,
        w,
        PlatformInput::MouseMove(gpui_kit::MouseMoveEvent {
            position: before.center(),
            pressed_button: None,
            modifiers: Default::default(),
        }),
    );
    settle(cx, w, 4);
    assert_eq!(bounds(cx, star), Some(before), "hovered: drawn in its slot, nothing moved");
    click_at(cx, w, before.center());
    settle(cx, w, 4);
    view.read_with(cx, |v, _| {
        assert_eq!(v.model.favs.list.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), [id.as_str()]);
        assert!(v.model.named.resuming.is_none(), "the star's click is its own: the row did not open");
        let rail = &v.rail.rail;
        assert!(matches!(rail.items[0], Item::Favs { open: true, .. }), "FAVOURITES leads the rail");
        let rows: Vec<u64> =
            rail.items.iter().filter_map(|i| if let Item::Row(r) | Item::Hist(r) = i { Some(rail.rows[*r].key) } else { None }).collect();
        assert_eq!(rows.iter().filter(|k| **k == key).count(), 1, "in FAVOURITES only");
        assert!(matches!(rail.items[1], Item::Row(r) if rail.rows[r].key == key && rail.rows[r].fav == Some(0)));
    });
    // The key: the session on screen; not while typing (the composer's
    // Ctrl+D is a TUI hand's delete, never a toggle).
    let shown = view.read_with(cx, |v, _| v.model.tabs.current().and_then(|t| t.session.clone()).expect("the demo's session"));
    let fav_key = if cfg!(target_os = "macos") { "cmd-d" } else { "ctrl-d" };
    focus_composer(cx, w, &view);
    down(cx, w, fav_key, false);
    up(cx, w, "d");
    settle(cx, w, 4);
    assert_eq!(view.read_with(cx, |v, _| v.model.favs.list.len()), 1, "typing: the key is the composer's");
    cx.update_window(w, |_, window, cx| view.update(cx, |v, cx| window.focus(&v.focus, cx))).expect("the window");
    settle(cx, w, 2);
    down(cx, w, fav_key, false);
    up(cx, w, "d");
    settle(cx, w, 4);
    let live_key = view.read_with(cx, |v, _| {
        assert_eq!(v.model.favs.list.len(), 2);
        let s = v.model.sessions.iter().find(|s| s.id == shown).unwrap();
        assert!(v.model.fav_of(s).is_some(), "Ctrl+D favourited the session on screen");
        assert!(!s.pinned, "a favourite is not the daemon's pin");
        crate::rail::key_of(&shown)
    });
    // FAVOURITES folds from its head and opens again, drawn each time.
    view.update(cx, |v, cx| {
        v.rail_list.scroll_to_reveal_item(0);
        cx.notify();
    });
    settle(cx, w, 4);
    let head = bounds(cx, "favs-head").expect("the group's head");
    let row_shown = |cx: &mut TestAppContext| {
        view.read_with(cx, |v, _| v.rail.rail.items.iter().any(|i| matches!(i, Item::Row(r) if v.rail.rail.rows[*r].key == key)))
    };
    click_at(cx, w, head.center());
    settle(cx, w, 4);
    assert!(view.read_with(cx, |v, _| v.panels.favs_closed));
    assert!(!row_shown(cx), "folded: its head only");
    assert_eq!(bounds(cx, "favs-head"), Some(head));
    click_at(cx, w, head.center());
    settle(cx, w, 4);
    assert!(view.read_with(cx, |v, _| !v.panels.favs_closed) && row_shown(cx), "open again");
    // Its filled star, drawn at rest; a click unfavourites it.
    let star: &'static str = Box::leak(format!("star {live_key}").into_boxed_str());
    let b = bounds(cx, star).expect("the favourite's star");
    click_at(cx, w, b.center());
    settle(cx, w, 4);
    view.read_with(cx, |v, _| {
        assert_eq!(v.model.favs.list.len(), 1);
        assert!(v.model.is_shown(&shown), "still the session on screen: nothing opened or closed");
    });
}

// ── chrome motion (polish.md Q4: fluid.md F2, motion.md PR 5) ─────────

/// A left-button press at `at`, and its release.
fn press_at(cx: &mut TestAppContext, w: AnyWindowHandle, at: gpui_kit::Point<gpui_kit::Pixels>) {
    let ev = gpui_kit::MouseDownEvent {
        button: gpui_kit::MouseButton::Left,
        position: at,
        modifiers: Default::default(),
        click_count: 1,
        first_mouse: false,
    };
    event(cx, w, PlatformInput::MouseDown(ev));
}

fn release_at(cx: &mut TestAppContext, w: AnyWindowHandle, at: gpui_kit::Point<gpui_kit::Pixels>) {
    let ev = gpui_kit::MouseUpEvent { button: gpui_kit::MouseButton::Left, position: at, modifiers: Default::default(), click_count: 1 };
    event(cx, w, PlatformInput::MouseUp(ev));
}

/// fluid.md §3.9: a press shows in the next frame, whatever the level: the
/// content 1 px down (Full) and the press fill; let go, it springs back on
/// `flick` and comes to rest where it was. Reduced: the fill, no travel.
#[gpui_kit::test]
fn press_feedback_is_same_frame(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    let id = view.read_with(cx, |v, _| v.model.tabs.iter().next().map(|t| t.id)).expect("a tab");
    let title = format!("tab {id} title");
    let leak: &'static str = Box::leak(title.into_boxed_str());
    let y = |cx: &mut TestAppContext| VisualTestContext::from_window(w, cx).debug_bounds(leak).expect("the tab's title").origin.y;
    for level in [crate::motion::Level::Full, crate::motion::Level::Reduced] {
        set_motion(cx, &view, level);
        settle(cx, w, 8);
        let y0 = y(cx);
        let at = VisualTestContext::from_window(w, cx).debug_bounds(leak).expect("the title").center();
        press_at(cx, w, at);
        frame(cx, &view);
        let travel = if level == crate::motion::Level::Full { 1. } else { 0. };
        assert_eq!(y(cx), y0 + px(travel), "{level:?}: the press, in the next frame");
        let key = view.read_with(cx, |v, _| v.fx.pressed(crate::motion::key(crate::tabstrip::TOUCH, id)));
        assert_eq!(key, (1., travel), "{level:?}: held: the press fill, {travel} px down");
        release_at(cx, w, at);
        for _ in 0..60 {
            wait(cx, w, &view, 7);
        }
        assert_eq!(y(cx), y0, "{level:?}: let go, back where it was");
        assert_eq!(view.read_with(cx, |v, _| v.fx.pressed(crate::motion::key(crate::tabstrip::TOUCH, id))), (0., 0.), "{level:?}: at rest");
    }
}

/// fluid.md §3.5: the visible tab's fill and bar are one quad pair that
/// springs to a newly visible tab; a switch mid-flight retargets from where
/// it is (never a jump), and it lands exactly on the tab.
#[gpui_kit::test]
fn indicator_retargets(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    view.update(cx, |v, cx| {
        v.client = None;
        let ids: Vec<String> = v.model.sessions.iter().map(|s| s.id.clone()).take(3).collect();
        for id in ids {
            v.open_tab(crate::app::Open::Attach { session_id: id, fallback: None, open: false }, crate::tabs::Place::New);
        }
        cx.notify();
    });
    settle(cx, w, 8);
    wait(cx, w, &view, 600);
    let n = view.read_with(cx, |v, _| v.model.tabs.len());
    assert!(n >= 3, "three tabs: {n}");
    let ind = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.fx.spring_at(crate::tabstrip::IND_X));
    let focus = |cx: &mut TestAppContext, ix: usize| {
        view.update(cx, |v, cx| {
            v.focus_tab(ix);
            cx.notify();
        });
        cx.run_until_parked();
    };
    let x_of = |cx: &mut TestAppContext, ix: usize| {
        let id = view.read_with(cx, |v, _| v.model.tabs.iter().nth(ix).map(|t| t.id)).expect("the tab");
        let s: &'static str = Box::leak(format!("tab {id}").into_boxed_str());
        f32::from(VisualTestContext::from_window(w, cx).debug_bounds(s).expect("the tab").origin.x)
    };
    focus(cx, 0);
    wait(cx, w, &view, 600);
    let start = ind(cx).expect("the indicator");
    focus(cx, n - 1);
    let mut last = start;
    for _ in 0..8 {
        wait(cx, w, &view, 7);
        last = ind(cx).expect("moving");
    }
    let far = x_of(cx, n - 1) - x_of(cx, 0);
    assert!(last > start + 1. && last < start + far - 1., "mid-flight: {start} → {last} of {far}");
    // Retarget to the middle tab: the next frame starts where it was.
    focus(cx, 1);
    wait(cx, w, &view, 7);
    let next = ind(cx).expect("moving");
    // At most a frame of the spring's own travel (its top speed over the
    // whole way is far × ω / e, ω = 2π / snap): 7 ms of it, with room.
    let step = far * std::f32::consts::TAU / crate::motion::SNAP_RESPONSE / std::f32::consts::E * 0.007 * 1.5;
    assert!((next - last).abs() < step, "no jump on a retarget: {last} → {next} (a frame is at most {step})");
    for _ in 0..80 {
        wait(cx, w, &view, 7);
    }
    assert_eq!(ind(cx).expect("at rest"), x_of(cx, 1) - x_of(cx, 0) + start, "lands on the tab");
    assert!(!motion_pending(cx, &view).0, "and asks for nothing more");
}

/// fluid.md §3.5 "Close" (finish gate 1): closing the visible tab, the
/// indicator stays in the closed slot while its neighbour slides in under
/// it; it never runs right and back.
#[gpui_kit::test]
fn closing_a_tab_keeps_the_indicator_in_its_slot(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    view.update(cx, |v, cx| {
        v.client = None;
        let ids: Vec<String> = v.model.sessions.iter().map(|s| s.id.clone()).take(3).collect();
        for id in ids {
            v.open_tab(crate::app::Open::Attach { session_id: id, fallback: None, open: false }, crate::tabs::Place::New);
        }
        cx.notify();
    });
    settle(cx, w, 8);
    wait(cx, w, &view, 600);
    let n = view.read_with(cx, |v, _| v.model.tabs.len());
    assert!(n >= 3, "three tabs: {n}");
    let ind = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.fx.spring_at(crate::tabstrip::IND_X));
    view.update(cx, |v, cx| {
        v.focus_tab(1);
        cx.notify();
    });
    cx.run_until_parked();
    wait(cx, w, &view, 600);
    let slot = ind(cx).expect("the indicator");
    view.update(cx, |v, cx| {
        v.close_tab(1);
        cx.notify();
    });
    cx.run_until_parked();
    let mut xs = vec![];
    for _ in 0..60 {
        wait(cx, w, &view, 7);
        xs.push(ind(cx).unwrap_or(slot));
    }
    let worst = xs.iter().copied().fold(f32::MIN, f32::max);
    assert!(worst <= slot + 0.5, "the indicator never leaves the closed slot to the right: {slot} → max {worst} ({xs:?})");
    let quad = view.read_with(cx, |v, _| v.model.tabs.active());
    assert!(quad.is_some(), "a tab is shown");
    assert!(!motion_pending(cx, &view).0, "settled");
}

/// fluid.md §3.8 (M3): a new tab's width springs open from 0; settled, a
/// later change of width (the window widening, every tab with it) is taken
/// at once, in the first frame, by every tab: none springs again.
#[gpui_kit::test]
fn a_settled_tab_takes_a_new_width_at_once(cx: &mut TestAppContext) {
    use gpui_kit::VisualTestContext;
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    let ids = view.update(cx, |v, cx| {
        v.client = None;
        v.panels.rail_width = Some(220);
        v.panels.right_width = Some(413);
        let ids: Vec<String> = v.model.sessions.iter().map(|s| s.id.clone()).collect();
        for id in ids {
            v.open_tab(crate::app::Open::Attach { session_id: id, fallback: None, open: false }, crate::tabs::Place::New);
        }
        cx.notify();
        v.model.tabs.iter().map(|t| t.id).collect::<Vec<_>>()
    });
    // Each tab's drawn width (its spring; the tab itself is laid out at
    // the final width, clipped to this while it opens).
    let widths = |vcx: &mut VisualTestContext| {
        view.read_with(vcx, |v, _| {
            ids.iter().filter_map(|id| v.fx.spring_at(crate::motion::key(crate::tabstrip::OPEN_W, *id))).collect::<Vec<_>>()
        })
    };
    let mut vcx = VisualTestContext::from_window(w, cx);
    vcx.run_until_parked();
    // Opening: springs from 0.
    let opening = widths(&mut vcx);
    wait(&mut vcx, w, &view, 600);
    let settled = widths(&mut vcx);
    assert!(!opening.is_empty() && opening.iter().zip(&settled).all(|(a, b)| a < b), "they opened on a spring: {opening:?} → {settled:?}");
    vcx.simulate_resize(size(px(1000.), px(900.)));
    vcx.run_until_parked();
    let first = widths(&mut vcx);
    wait(&mut vcx, w, &view, 600);
    let end = widths(&mut vcx);
    assert_ne!(settled, end, "the widening changed the tabs' width");
    // The tabs still drawn (a tab the narrower strip no longer shows keeps
    // its last width until a frame without it: the rail's spinners now
    // draw only the beat layer, not the app's frame).
    let first: Vec<f32> = first.iter().copied().filter(|x| end.contains(x)).collect();
    assert_eq!(first, end, "the new width in the first frame: no spring");
}

/// fluid.md §3.6: a switch within 150 ms of the last is instant (holding
/// ↓ never stacks transitions); one after it plays.
#[gpui_kit::test]
fn rapid_switch_is_instant(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    view.update(cx, |v, _| v.client = None);
    open_idle_cpu(cx, &view, crate::tabs::Place::New);
    wait(cx, w, &view, 600);
    let focus = |cx: &mut TestAppContext, ix: usize| {
        view.update(cx, |v, cx| {
            v.focus_tab(ix);
            cx.notify();
        });
        cx.run_until_parked();
        frame(cx, &view);
    };
    focus(cx, 0);
    assert!(view.read_with(cx, |v, _| v.layer.is_some()), "a switch on its own plays");
    wait(cx, w, &view, 100);
    focus(cx, 1);
    assert!(view.read_with(cx, |v, _| v.layer.is_none()), "within 150 ms of the last: instant");
    wait(cx, w, &view, 400);
    focus(cx, 0);
    assert!(view.read_with(cx, |v, _| v.layer.is_some()), "after it: plays");
}

/// fluid.md §3.10: a menu the keyboard opened is there at once (a keyboard
/// user waits on its first item); one the pointer opened enters.
#[gpui_kit::test]
fn keyboard_menu_instant(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    menu_on(cx, w, &view, Act::Open);
    let born = view.read_with(cx, |v, _| v.menu.as_ref().map(|m| m.0.born));
    assert_eq!(born, Some(None), "opened from the keyboard: no enter");
    view.update(cx, |v, _| v.close_menu());
    cx.run_until_parked();
    let key = view.read_with(cx, |v, _| v.rail.rail.rows.iter().find(|r| r.live.is_some()).map(|r| r.key)).expect("a live row");
    cx.update_window(w, |_, window, cx| {
        view.update(cx, |v, cx| v.open_session_menu(crate::app::SessionAt::Row(key), Default::default(), window, cx))
    })
    .expect("the window");
    cx.run_until_parked();
    let (strength, rise) = view.read_with(cx, |v, _| v.fx.enter(v.smenu.as_ref().and_then(|m| m.0.born)));
    assert!(strength < 1. && rise > 0., "the pointer's enters: {strength} {rise}");
    wait(cx, w, &view, 400);
    let (strength, rise) = view.read_with(cx, |v, _| v.fx.enter(v.smenu.as_ref().and_then(|m| m.0.born)));
    assert_eq!((strength, rise), (1., 0.), "and lands");
}

/// motion.md §6.4 / fluid.md §3.10: the rail readout moving to the next row
/// within 300 ms shows at once, at full strength, and follows from the row
/// before (no re-enter).
#[gpui_kit::test]
fn overlay_replace_does_not_reenter(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    let keys: Vec<u64> = view.read_with(cx, |v, _| v.rail.rail.rows.iter().filter(|r| r.live.is_some()).map(|r| r.key).take(2).collect());
    assert_eq!(keys.len(), 2, "two live rows");
    view.update(cx, |v, cx| v.hover_row(keys[0], true, cx));
    wait(cx, w, &view, 500);
    assert_eq!(view.read_with(cx, |v, _| v.readout), Some(keys[0]), "the first row's readout");
    assert!(view.read_with(cx, |v, _| v.readout_at.is_some()), "it entered");
    view.update(cx, |v, cx| {
        v.hover_row(keys[0], false, cx);
        v.hover_row(keys[1], true, cx);
    });
    frame(cx, &view);
    let (now, at, from) = view.read_with(cx, |v, _| (v.readout, v.readout_at, v.rail_fx.readout_dy));
    assert_eq!(now, Some(keys[1]), "the next row's, at once (no 400 ms wait)");
    assert_eq!(at, None, "no enter: full strength from its first frame");
    assert!(from != 0., "it follows from the row before: {from}");
    for _ in 0..60 {
        wait(cx, w, &view, 7);
    }
    assert_eq!(view.read_with(cx, |v, _| v.rail_fx.readout_dy), 0., "and lands on its row");
}

/// motion.md §6.4: a reorder under a held rail row waits for the release:
/// a row never moves away from a click. Only the order waits: a row's
/// content (here a session's new title) is today's while held.
#[gpui_kit::test]
fn rail_reorder_waits_for_release(cx: &mut TestAppContext) {
    let (_w, view, _events) = open(cx);
    let order = |cx: &mut TestAppContext| {
        view.read_with(cx, |v, _| v.rail.rail.items.iter().map(|i| crate::rail::item_key(&v.rail.rail, i)).collect::<Vec<_>>())
    };
    let (key, id) = view
        .read_with(cx, |v, _| v.rail.rail.rows.iter().find_map(|r| r.live.map(|i| (r.key, v.model.sessions[i].id.clone()))))
        .expect("a live row");
    let before = order(cx);
    view.update(cx, |v, cx| {
        v.fx.press_down(crate::motion::key(crate::chrome::RAIL_TOUCH, key));
        // Another session is pinned (it moves up), and the held one renamed.
        let other =
            v.model.sessions.iter_mut().rev().find(|s| s.id != id && !s.pinned && s.lifecycle != "ending").expect("another session");
        other.pinned = true;
        let s = v.model.sessions.iter_mut().find(|s| s.id == id).expect("its session");
        (s.name, s.title) = (None, Some("renamed while held".into()));
        v.model.rail_gen += 1;
        cx.notify();
    });
    frame(cx, &view);
    assert_eq!(order(cx), before, "held: the order as it was");
    let title = view.read_with(cx, |v, _| v.rail.rail.rows.iter().find(|r| r.key == key).map(|r| r.title.to_string()));
    assert_eq!(title.as_deref(), Some("renamed while held"), "held: the row's content is today's");
    view.update(cx, |v, cx| {
        v.fx.press_up();
        cx.notify();
    });
    frame(cx, &view);
    assert_ne!(order(cx), before, "let go: the change lands");
}

/// Flip (motion.md §6.4) in a splice's own frame, before the list lays out
/// again: a row that moved starts where it was and slides; the rows that
/// didn't move get no offset; the next frame (laid out) agrees with where
/// the splice frame said each row would be.
#[gpui_kit::test]
fn rail_reorder_flips_moved_rows_only(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    settle(cx, w, 4);
    // Every live row's key and top, as laid out.
    let tops = |cx: &mut TestAppContext| {
        view.read_with(cx, |v, _| {
            let rail = &v.rail.rail;
            rail.items
                .iter()
                .enumerate()
                .filter_map(|(ix, it)| Some((crate::chrome::row_key(rail, it)?, crate::chrome::item_y(&v.rail_list, ix)?.0)))
                .collect::<std::collections::HashMap<u64, f32>>()
        })
    };
    // Only rows on screen were seen where they were.
    let view_h = view.read_with(cx, |v, _| f32::from(v.rail_list.viewport_bounds().size.height));
    let before: std::collections::HashMap<u64, f32> = tops(cx).into_iter().filter(|&(_, y)| y < view_h).collect();
    // The lowest OPEN row on screen is pinned: it moves up into PINNED.
    let moved = view
        .read_with(cx, |v, _| {
            v.model
                .sessions
                .iter()
                .filter(|s| !s.pinned && s.lifecycle != "ending")
                .filter_map(|s| before.get(&crate::rail::key_of(&s.id)).map(|y| (*y, s.id.clone())))
                .max_by(|a, b| a.0.total_cmp(&b.0))
                .map(|(_, id)| id)
        })
        .expect("an open session on screen");
    view.update(cx, |v, cx| {
        let s = v.model.sessions.iter_mut().find(|s| s.id == moved).expect("its session");
        s.pinned = true;
        v.model.rail_gen += 1;
        cx.notify();
    });
    cx.run_until_parked();
    let offsets = view.read_with(cx, |v, _| v.rail_fx.offsets.clone());
    let after = tops(cx);
    let key = crate::rail::key_of(&moved);
    let mut slid = 0;
    for (k, y) in &after {
        let Some(was) = before.get(k) else { continue };
        let off = offsets.get(k).copied().unwrap_or(0.);
        if (was - y).abs() < 0.5 {
            assert!(off.abs() < 0.5, "a row that didn't move has no offset ({off})");
        } else {
            let want = (was - y).clamp(-crate::chrome::FLIP_CAP, crate::chrome::FLIP_CAP);
            assert!((off - want).abs() < 1., "a moved row starts where it was: {off} vs {want} (key {k:x})");
            slid += 1;
        }
    }
    assert!(slid >= 2 && before.get(&key) != after.get(&key), "the moved row and those it passed slide ({slid})");
    // The row that travels farthest is drawn over the ones it passes.
    let leads = view.read_with(cx, |v, _| v.rail_fx.leads.keys().copied().collect::<Vec<_>>());
    assert_eq!(leads, vec![key], "the moved row leads");
    wait(cx, w, &view, 400);
    assert!(view.read_with(cx, |v, _| v.rail_fx.offsets.is_empty()), "settled");
}

/// motion.md §6.8: a theme change cross-fades the palette over `slow`
/// under Full (its first frame the old colours, then between, then the
/// new); Reduced and Off change it at once.
#[gpui_kit::test]
fn a_theme_change_cross_fades(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    let ink = |cx: &mut TestAppContext| cx.update(|cx| crate::theme::palette(cx).canvas);
    let kit = |cx: &mut TestAppContext| cx.update(|cx| Theme::global(cx).colors.background);
    let to = |cx: &mut TestAppContext, name: &str| {
        cx.update(|cx| {
            let raw = crate::theme::builtin(name).expect("built in").1;
            crate::theme::update(cx, |c| {
                c.theme = name.into();
                c.raw = raw;
            });
        });
        frame(cx, &view);
    };
    let fading = |cx: &mut TestAppContext| cx.update(|cx| cx.global::<crate::theme::Scheme>().fading().is_some());
    wait(cx, w, &view, 400);
    // Dark to dark: the palette blends; the kit and the text views follow
    // on the last frame.
    let (old, old_kit) = (ink(cx), kit(cx));
    to(cx, "night-city");
    let new = cx.update(|cx| cx.global::<crate::theme::Scheme>().p.canvas);
    assert_ne!(old, new, "night-city's canvas is another colour");
    assert_eq!(ink(cx), old, "its first frame: the old palette");
    assert_eq!(kit(cx), old_kit, "the kit waits for the last frame");
    wait(cx, w, &view, 180);
    let mid = ink(cx);
    assert!(mid != old && mid != new, "between: {mid:?}");
    // Re-gate 5: the text views' colours (links, code spans) move with it,
    // a frame behind at most, never after it.
    let text_t = |cx: &mut TestAppContext| cx.update(|cx| cx.global::<crate::theme::Scheme>().text_t.get());
    let tt = text_t(cx);
    assert!(tt > 0.2 && tt < 0.8, "the text views' style mid-fade: {tt}");
    wait(cx, w, &view, 400);
    assert_eq!(ink(cx), new, "then the new one");
    assert_eq!(text_t(cx), 1., "the text views' style at the end");
    assert_ne!(kit(cx), old_kit, "the kit, synced on the last frame");
    assert!(!fading(cx));
    // Dark to light: at once (text and ground would cross mid-fade).
    to(cx, "paper");
    let paper = cx.update(|cx| cx.global::<crate::theme::Scheme>().p.canvas);
    assert_eq!(ink(cx), paper, "a polarity change: at once");
    assert!(!fading(cx));
    // A text size: no colour changes, no fade.
    cx.update(|cx| crate::theme::update(cx, |c| c.text = 18));
    frame(cx, &view);
    assert!(!fading(cx), "a text size never fades");
    to(cx, "nord");
    for level in [crate::motion::Level::Reduced, crate::motion::Level::Off] {
        set_motion(cx, &view, level);
        to(cx, "night-city");
        let nc = cx.update(|cx| cx.global::<crate::theme::Scheme>().p.canvas);
        assert_eq!(ink(cx), nc, "{level:?}: at once");
        to(cx, "nord");
    }
}

/// The Usage charts reveal on their first board and on another range,
/// never on a refresh of the same (motion.md §6.8); at Full only: Reduced
/// and Off show them whole.
#[gpui_kit::test]
fn chart_reveal_skipped_when_reduced(cx: &mut TestAppContext) {
    use crate::usage::{Board, Range, Raw};
    let (w, view, _events) = open(cx);
    let board = |range: Range| {
        let path = format!("{}/fixtures/usage/all.json", env!("CARGO_MANIFEST_DIR"));
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        std::sync::Arc::new(Board::build(&Raw::from_fixture(&v).unwrap(), None, range, 1_794_654_869_189))
    };
    let show = |cx: &mut TestAppContext, b: Option<Range>| {
        view.update(cx, |v, _| {
            if let Some(r) = b {
                v.usage.board = Some(board(r));
            }
            v.usage_reveal()
        })
    };
    set_motion(cx, &view, crate::motion::Level::Full);
    settle(cx, w, 4);
    let first = show(cx, Some(Range::ALL[0]));
    assert!(first.at.is_some(), "the first board reveals");
    assert_eq!(first.column(0, 30), 0., "from the baseline");
    wait(cx, w, &view, 100);
    let r = show(cx, None);
    assert!(r.column(0, 30) > r.column(29, 30), "the left columns lead: {} {}", r.column(0, 30), r.column(29, 30));
    assert!(r.line() > 0. && r.line() < 1., "the lines draw on: {}", r.line());
    wait(cx, w, &view, chart_span());
    assert_eq!(show(cx, None), crate::chart::Reveal::default(), "over: shown whole");
    assert_eq!(show(cx, Some(Range::ALL[0])), crate::chart::Reveal::default(), "a refresh of the same range: no reveal");
    assert!(show(cx, Some(Range::ALL[1])).at.is_some(), "another range reveals");
    for level in [crate::motion::Level::Reduced, crate::motion::Level::Off] {
        set_motion(cx, &view, level);
        settle(cx, w, 4);
        wait(cx, w, &view, chart_span());
        let k = if level == crate::motion::Level::Reduced { 2 } else { 0 };
        assert_eq!(show(cx, Some(Range::ALL[k])), crate::chart::Reveal::default(), "{level:?}: shown whole");
    }
}

fn chart_span() -> u64 {
    crate::chart::Reveal::SPAN.as_millis() as u64 + 10
}

/// A saved setting's value afterglows (motion.md §6.8: the confirmation, no
/// toast): a THIS APP value once its write is due, a daemon one when the
/// write is answered; never on first sight, never under Off.
#[gpui_kit::test]
fn saved_value_afterglows(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    settle(cx, w, 4);
    let id: gpui_kit::SharedString = "field-dash-url".into();
    let k = |cx: &mut TestAppContext, value: &str, sending: bool| view.read_with(cx, |v, _| v.saved_glow(&id, value, sending));
    // A THIS APP key waiting for its write (`save_app` put it there).
    view.update(cx, |v, _| {
        v.settings.app_dirty.insert("dash_url", None);
    });
    // Read outside a frame: at the motion clock, moved here alone.
    let wait = |cx: &mut TestAppContext, _: AnyWindowHandle, view: &Entity<AppView>, ms: u64| {
        view.read_with(cx, |v, _| v.fx.set_clock(v.fx.clock() + std::time::Duration::from_millis(ms)));
    };
    assert_eq!(k(cx, "a", false), 0., "first sight");
    assert_eq!(k(cx, "b", false), 0., "changed: its write is still to come");
    wait(cx, w, &view, crate::app::SAVE_DELAY.as_millis() as u64 + 50);
    assert_eq!(k(cx, "b", false), 0., "not written yet (no guess at when)");
    // Its write finished (the one that saved it, or a later one).
    view.update(cx, |v, _| v.settings.app_wrote = Some((u64::MAX, v.fx.now())));
    let lit = k(cx, "b", false);
    assert!(lit > 0.5, "written: lit ({lit})");
    wait(cx, w, &view, crate::motion::GLOW.as_millis() as u64);
    assert_eq!(k(cx, "b", false), 0., "settled");
    assert_eq!(k(cx, "c", true), 0., "sending");
    wait(cx, w, &view, 50);
    assert_eq!(k(cx, "c", false), 1., "answered: lit at once");
    // Nit (cross-client): a change that isn't this app's pending write (a
    // daemon value another client set) glows as it lands, not on this
    // app's next unrelated write.
    view.update(cx, |v, _| v.settings.app_dirty.clear());
    let other: gpui_kit::SharedString = "field-model".into();
    let k2 = |cx: &mut TestAppContext, value: &str| view.read_with(cx, |v, _| v.saved_glow(&other, value, false));
    assert_eq!(k2(cx, "x"), 0., "first sight");
    assert_eq!(k2(cx, "y"), 1., "another client's change: lit at once");
    // Settings not shown (this test's window shows a session): what it
    // showed is forgotten, so a value changed meanwhile is first sight.
    frame(cx, &view);
    assert!(view.read_with(cx, |v, _| v.settings.saved.borrow().is_empty()), "hidden: forgotten");
    assert_eq!(k(cx, "e", false), 0., "reopened: first sight, not a save");
    set_motion(cx, &view, crate::motion::Level::Off);
    settle(cx, w, 4);
    assert_eq!(k(cx, "d", true), 0.);
    assert_eq!(k(cx, "d", false), 0., "Off: none");
}

/// The link's signatures (motion.md §5.1, §5.3): a drop tears for two
/// frames (the connection cell +3 px, then −2), the palette drains and
/// holds while down; back, it scans and restores. Every drained role keeps
/// its contrast. Reduced: no tear, no scan, a quick drain. Offline: never.
#[gpui_kit::test]
fn the_link_drops_drains_and_scans_back(cx: &mut TestAppContext) {
    use crate::model::ConnState;
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    let conn = |cx: &mut TestAppContext, c: ConnState| {
        view.update(cx, |v, cx| {
            v.model.conn = c;
            cx.notify();
        });
        cx.run_until_parked();
    };
    let drain = |cx: &mut TestAppContext| cx.update(|cx| cx.global::<crate::theme::Scheme>().drain.get());
    let run = |cx: &mut TestAppContext| cx.update(|cx| crate::theme::palette(cx).run);
    let lit = run(cx);
    // Offline (the demo's synthetic transport): nothing, whatever it says.
    conn(cx, ConnState::Connected("x".into()));
    conn(cx, ConnState::Reconnecting);
    assert_eq!(view.read_with(cx, |v, _| v.link.glitch), None, "offline: never");
    view.update(cx, |v, _| v.synthetic = false);
    conn(cx, ConnState::Connected("x".into()));
    assert!(view.read_with(cx, |v, _| v.link.scan.is_some()), "the first hello scans");
    wait(cx, w, &view, 700);
    assert!(view.read_with(cx, |v, _| v.link.scan.is_none()), "the scan is over");
    conn(cx, ConnState::Reconnecting);
    assert_eq!(view.read_with(cx, |v, _| v.conn_dy), 0., "the grace: nothing yet");
    assert_eq!(drain(cx), 0.);
    wait_past(cx, w, &view, crate::motion::LINK_GRACE.as_millis() as u64);
    // (The tear's two frames, to the millisecond: motion.rs
    // `the_link_tears_after_its_grace`.)
    assert!(view.read_with(cx, |v, _| v.link.tore.is_some()), "past the grace: the tear");
    wait(cx, w, &view, 500);
    assert_eq!(view.read_with(cx, |v, _| v.conn_dy), 0., "the tear is over");
    assert_eq!(drain(cx), 1., "drained");
    let grey = run(cx);
    assert_ne!(grey, lit, "the live colours drain");
    let (l0, l1) = (crate::theme::luminance_hsla(lit), crate::theme::luminance_hsla(grey));
    assert!((l0 - l1).abs() < 0.01, "at their own luminance: {l0} {l1}");
    wait(cx, w, &view, 2000);
    assert_eq!(drain(cx), 1., "held while down");
    // Holding asks for no display frame (the word's dots step on the beat).
    assert!(!motion_pending(cx, &view).0, "holding draws nothing");
    conn(cx, ConnState::Connected("x".into()));
    assert!(view.read_with(cx, |v, _| v.link.scan.is_some()), "back: it scans");
    // FG2: the Scan's line is drawn live from its first frame, while the
    // palette is still drained.
    let (line, live, drained) = (
        view.read_with(cx, |v, _| v.scan_line),
        cx.update(|cx| crate::theme::live_palette(cx).run_mark_ch),
        cx.update(|cx| crate::theme::palette(cx).run_mark_ch),
    );
    assert_eq!(line, Some(live), "the Scan in the live colour");
    assert_ne!(live, drained, "(while the palette is drained)");
    wait(cx, w, &view, 400);
    assert_eq!(drain(cx), 0., "restored");
    assert_eq!(run(cx), lit);
    wait(cx, w, &view, 400);
    assert!(!motion_pending(cx, &view).0, "settled: no display frames");
    // Reduced: no tear, no scan; the drain over `quick`.
    set_motion(cx, &view, crate::motion::Level::Reduced);
    settle(cx, w, 4);
    conn(cx, ConnState::Reconnecting);
    wait(cx, w, &view, crate::motion::LINK_GRACE.as_millis() as u64);
    assert_eq!(view.read_with(cx, |v, _| v.conn_dy), 0., "Reduced: no tear");
    wait(cx, w, &view, 130);
    assert_eq!(drain(cx), 1., "Reduced: a quick drain");
    conn(cx, ConnState::Connected("x".into()));
    assert!(view.read_with(cx, |v, _| v.link.scan.is_none()), "Reduced: no scan");
    wait(cx, w, &view, 130);
    assert_eq!(drain(cx), 0.);
    // Off: none.
    set_motion(cx, &view, crate::motion::Level::Off);
    settle(cx, w, 4);
    conn(cx, ConnState::Reconnecting);
    wait(cx, w, &view, crate::motion::LINK_GRACE.as_millis() as u64 + 500);
    assert_eq!(drain(cx), 0., "Off: none");
    assert_eq!(motion_pending(cx, &view), (false, None), "Off: the dots hold too");
}

/// Re-gate 1 (motion.md §5.3 "over `slow` 360 `inout`"): the drain and
/// the restore run their whole tween at frame rate. From the tear's end
/// every 7 ms frame asks for the next and moves the palette on (no step
/// over a quarter of the way) until it is all the way, and the app's frame
/// (the composer's bar with it) draws each step.
#[gpui_kit::test]
fn the_drain_and_the_restore_run_every_frame(cx: &mut TestAppContext) {
    let (w, view) = link_up(cx);
    let drain = |cx: &mut TestAppContext| cx.update(|cx| cx.global::<crate::theme::Scheme>().drain.get());
    let bar = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.composer_bar.get());
    let frames = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.fx.frames());
    let lit = bar(cx).expect("the composer's bar is drawn");
    let run = |cx: &mut TestAppContext, to: f32, what: &str| {
        let mut ks = vec![drain(cx)];
        let mut bars = vec![bar(cx)];
        for _ in 0..120 {
            if ks.last() == Some(&to) {
                break;
            }
            let (frame, timer) = motion_pending(cx, &view);
            assert!(frame || timer.is_some(), "{what}: the window stopped asking for frames at {:?}", ks.last());
            let f0 = frames(cx);
            wait(cx, w, &view, 7);
            ks.push(drain(cx));
            bars.push(bar(cx));
            assert!(frames(cx) > f0, "{what}: no frame drawn at {:?}", ks.last());
        }
        assert_eq!(ks.last(), Some(&to), "{what}: all the way ({ks:?})");
        let moving: Vec<f32> = ks.iter().copied().filter(|k| *k > 0. && *k < 1.).collect();
        assert!(moving.len() >= 360 / 7 - 4, "{what}: at frame rate, {} frames between ({ks:?})", moving.len());
        for p in ks.windows(2) {
            assert!((p[1] - p[0]).abs() <= 0.25, "{what}: a step over a quarter ({} → {})", p[0], p[1]);
            assert!(if to > 0. { p[1] >= p[0] } else { p[1] <= p[0] }, "{what}: one way ({} → {})", p[0], p[1]);
        }
        let changes = bars.windows(2).filter(|p| p[0] != p[1]).count();
        assert!(changes >= moving.len(), "{what}: the composer's bar moves with it ({changes} of {} frames)", moving.len());
    };
    status(cx, &view, synaps_client::Status::Reconnecting { why: "daemon closed the connection".into() });
    wait_past(cx, w, &view, crate::motion::LINK_GRACE.as_millis() as u64);
    assert!(view.read_with(cx, |v, _| v.link.tore.is_some()), "lost");
    run(cx, 1., "the drain");
    assert_ne!(bar(cx), Some(lit), "the composer's bar drained");
    status(cx, &view, hello());
    run(cx, 0., "the restore");
    assert_eq!(bar(cx), Some(lit), "the composer's bar restored");
}

/// Re-gate r3 item 1, in what is painted: the composer bar's quad in the
/// window's rendered scene (the frame GPUI hands the renderer), read every
/// 7 ms frame through a loss and a return, through the cached app view
/// and its beat layer as the app opens them. It leaves the live colour on
/// the drain's first frame, moves on every frame after, never by more
/// than 8 % of the way, and is what render computed each frame.
#[gpui_kit::test]
fn the_drain_is_painted_every_frame(cx: &mut TestAppContext) {
    let (w, view) = link_up(cx);
    let painted = |cx: &mut TestAppContext| {
        let b = VisualTestContext::from_window(w, cx).debug_bounds("composer-bar").expect("the composer's bar");
        cx.update_window(w, |_, window, _| {
            let s = window.scale_factor();
            let near = |a: gpui_kit::ScaledPixels, b: gpui_kit::Pixels| (a.0 - f32::from(b) * s).abs() < 0.5;
            let hits: Vec<gpui_kit::Hsla> = window
                .painted_quads()
                .iter()
                .filter(|q| {
                    near(q.bounds.origin.x, b.origin.x)
                        && near(q.bounds.size.width, b.size.width)
                        && near(q.bounds.size.height, b.size.height)
                })
                .filter_map(|q| q.background.as_solid())
                .collect();
            assert_eq!(hits.len(), 1, "one quad is the composer's bar: {hits:?}");
            hits[0]
        })
        .expect("the window")
    };
    let computed = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.composer_bar.get()).expect("drawn");
    let lit = painted(cx);
    assert_eq!(lit, computed(cx), "live: painted as computed");
    let run = |cx: &mut TestAppContext, what: &str| {
        let from = painted(cx);
        let mut seen = vec![from];
        for _ in 0..80 {
            wait(cx, w, &view, 7);
            let c = painted(cx);
            assert_eq!(c, computed(cx), "{what}: painted as render computed it, frame {}", seen.len());
            seen.push(c);
            if !motion_pending(cx, &view).0 && motion_pending(cx, &view).1.is_none() && c != from {
                break;
            }
        }
        let to = *seen.last().expect("frames");
        let dist = |a: gpui_kit::Hsla, b: gpui_kit::Hsla| {
            let (a, b) = (gpui_kit::Rgba::from(a), gpui_kit::Rgba::from(b));
            ((a.r - b.r).powi(2) + (a.g - b.g).powi(2) + (a.b - b.b).powi(2)).sqrt()
        };
        let travel = dist(from, to);
        assert!(travel > 0.05, "{what}: it moves ({from:?} → {to:?})");
        let first = seen.iter().position(|c| *c != from).expect("it left");
        let last = seen.iter().position(|c| *c == to).expect("it got there");
        for (i, p) in seen.windows(2).enumerate() {
            // `inout`'s steepest is 2.86× linear: 6 % of the way in 7 ms.
            assert!(dist(p[0], p[1]) <= travel * 0.08 + 1e-4, "{what}: frame {i} → {}: a step over 8 % of the travel", i + 1);
            if i >= first && i < last {
                assert_ne!(p[0], p[1], "{what}: held at frame {} (moving {first}..{last})", i + 1);
            }
        }
        // `slow` (360 ms) at 7 ms frames: about 51 frames moving.
        assert!(last - first >= 360 / 7 - 4, "{what}: at frame rate ({first}..{last})");
        (first, to)
    };
    status(cx, &view, synaps_client::Status::Reconnecting { why: "daemon closed the connection".into() });
    wait_past(cx, w, &view, crate::motion::LINK_GRACE.as_millis() as u64);
    assert!(view.read_with(cx, |v, _| v.link.tore.is_some()), "lost");
    let (first, drained) = run(cx, "the drain");
    assert!(first <= 16, "the drain starts after the tear (100 ms): frame {first}");
    assert_ne!(drained, lit, "drained");
    status(cx, &view, hello());
    let (_, back) = run(cx, "the restore");
    assert_eq!(back, lit, "restored");
}

/// Re-gate r3: on a real loss too the words keep the connected words'
/// width (padded, or cut with an ellipsis), so NEEDS YOU, centred beside
/// them, never moves: "reconnecting ··", then the client's long error.
/// With a session attached (Shady #15 M1) its own connection re-attaches
/// on its own backoff, after the link's hello: "reattaching ···" holds
/// the width too, until it is attached again.
#[gpui_kit::test]
fn needs_you_holds_still_through_a_loss(cx: &mut TestAppContext) {
    use synaps_client::Status;
    let (w, view) = link_up(cx);
    let need = |cx: &mut TestAppContext| {
        let mut vcx = gpui_kit::VisualTestContext::from_window(w, cx);
        vcx.debug_bounds("attention").map(|b| f32::from(b.origin.x))
    };
    let up = need(cx);
    assert!(up.is_some(), "NEEDS YOU drawn");
    let seq = view.read_with(cx, |v, _| v.model.active.as_ref().expect("the demo session attached").attach_seq);
    view.update(cx, |v, cx| {
        let why = "daemon closed the connection".into();
        let _ = v.model.apply(synaps_client::Event::Reattaching { attach_seq: seq, session_id: crate::demo::SID.into(), reason: why });
        cx.notify();
    });
    cx.run_until_parked();
    wait(cx, w, &view, 7);
    assert_eq!(need(cx), up, "NEEDS YOU, the session re-attaching");
    let long = "connect /home/someone/.synaps-cli/run/daemon-desktopproto.sock: Connection refused (os error 111)";
    for (s, what) in [
        (Status::Reconnecting { why: "daemon closed the connection".into() }, "reconnecting"),
        (Status::Offline(long.into()), "the client's error"),
    ] {
        status(cx, &view, s);
        for _ in 0..3 {
            wait(cx, w, &view, 7);
            assert_eq!(need(cx), up, "NEEDS YOU on a loss: {what}");
        }
    }
    wait_past(cx, w, &view, crate::motion::LINK_GRACE.as_millis() as u64 + 600);
    assert_eq!(need(cx), up, "NEEDS YOU, lost and drained");
    status(cx, &view, hello());
    for _ in 0..3 {
        wait(cx, w, &view, 7);
        assert!(view.read_with(cx, |v, _| v.model.reattaching), "the link up first, the session still re-attaching");
        assert_eq!(need(cx), up, "NEEDS YOU, back, the session re-attaching");
    }
    view.update(cx, |v, cx| {
        let events = crate::demo::attach(crate::demo::SID, seq)
            .into_iter()
            .filter_map(|e| match e {
                synaps_client::Event::Attached { attach_seq, attached, mode, .. } => {
                    Some(synaps_client::Event::Attached { attach_seq, attached, mode, reconnect: true })
                }
                _ => None,
            })
            .collect();
        v.apply_batch(events, cx);
    });
    wait(cx, w, &view, 7);
    assert!(!view.read_with(cx, |v, _| v.model.reattaching), "attached again");
    assert_eq!(need(cx), up, "NEEDS YOU, back");
}

/// R2 (motion.md §5.1, §6.7): a real window starts at `Connecting`, so its
/// first frames see the link down; the first hello still scans, however
/// late it comes, and a launch that never connected tears nothing.
#[gpui_kit::test]
fn the_first_hello_scans_after_connecting(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    view.update(cx, |v, cx| {
        v.synthetic = false;
        v.link = Default::default();
        cx.notify();
    });
    cx.run_until_parked();
    status(cx, &view, synaps_client::Status::Connecting);
    wait(cx, w, &view, 3000);
    status(cx, &view, synaps_client::Status::Offline("connection refused".into()));
    wait(cx, w, &view, 3000);
    let (tore, scan) = view.read_with(cx, |v, _| (v.link.tore, v.link.scan));
    assert_eq!((tore, scan), (None, None), "never up yet: nothing to lose, nothing to scan");
    status(cx, &view, hello());
    assert!(view.read_with(cx, |v, _| v.link.scan.is_some()), "the first hello scans");
    assert_eq!(view.read_with(cx, |v, _| v.scan_line), Some(cx.update(|cx| crate::theme::live_palette(cx).run_mark_ch)));
}

/// A link set up for the link tests: Full, a live (not synthetic) link,
/// up, its first Scan over.
fn link_up(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<AppView>) {
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    view.update(cx, |v, _| v.synthetic = false);
    set_conn(cx, &view, crate::model::ConnState::Connected("x".into()), false);
    wait(cx, w, &view, 700);
    (w, view)
}

fn set_conn(cx: &mut TestAppContext, view: &Entity<AppView>, c: crate::model::ConnState, reload: bool) {
    view.update(cx, |v, cx| {
        v.model.conn = c;
        v.model.reload = reload;
        cx.notify();
    });
    cx.run_until_parked();
}

/// What the client says, as the model takes it (`Model::apply`): the
/// link's reason (`reload`, `reload_seq`) is the model's own mapping.
fn status(cx: &mut TestAppContext, view: &Entity<AppView>, s: synaps_client::Status) {
    view.update(cx, |v, cx| {
        let _ = v.model.apply(synaps_client::Event::Status(s));
        cx.notify();
    });
    cx.run_until_parked();
}

fn goodbye() -> synaps_client::Status {
    synaps_client::Status::Reconnecting { why: synaps_client::RELOADING.into() }
}

fn hello() -> synaps_client::Status {
    synaps_client::Status::Connected { daemon_version: "x".into(), profile: None, pid: 1, features: vec![] }
}

/// The link shows nothing of a reload: no tear, no drain, no Scan.
fn quiet(cx: &mut TestAppContext, view: &Entity<AppView>, what: &str) {
    let (dy, glitch, scan, lost) = view.read_with(cx, |v, _| (v.conn_dy, v.link.glitch, v.link.scan, v.link.lost));
    assert_eq!((dy, glitch.is_some(), scan.is_some(), lost), (0., false, false, false), "{what}");
    assert_eq!(cx.update(|cx| cx.global::<crate::theme::Scheme>().drain.get()), 0., "{what}: no drain");
}

/// Frames every 100 ms (the reconnect failing every second meanwhile)
/// until the link is lost: quiet before, and how long it was down (from
/// the grace's start) when it was lost. (The harness's clock can jump a
/// beat ahead: the grace is measured against the link's own `since`.)
fn until_lost(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) -> std::time::Duration {
    for n in 1..400 {
        let since = view.read_with(cx, |v, _| v.link.down.map(|d| d.0)).expect("down");
        wait(cx, w, view, 100);
        if n % 10 == 0 {
            status(cx, view, synaps_client::Status::Offline("connection refused".into()));
        }
        let (now, lost, tore) = view.read_with(cx, |v, _| (v.fx.now(), v.link.lost, v.link.tore));
        if lost {
            let tore = tore.expect("lost: the tear");
            return tore.saturating_duration_since(since);
        }
        quiet(cx, view, &format!("{:?} down", now.saturating_duration_since(since)));
    }
    panic!("never lost");
}

/// A daemon reload is a planned handover (its sessions come back): no
/// tear, no drain, no Scan, even when the new daemon takes 9 s (the
/// reconnect failing every `RELOAD_RETRY` meanwhile, Offline). Past
/// [`RELOAD_GRACE`] from its goodbye it is a loss.
#[gpui_kit::test]
fn a_reload_neither_tears_nor_scans(cx: &mut TestAppContext) {
    use synaps_client::Status;
    let (w, view) = link_up(cx);
    let retry = synaps_client::RELOAD_RETRY.as_millis() as u64;
    // An empty daemon (the sandbox's): the rail says "No open sessions".
    view.update(cx, |v, cx| {
        let _ = v.model.apply(synaps_client::Event::Sessions(Vec::new()));
        cx.notify();
    });
    cx.run_until_parked();
    let rail = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.rail.keys.clone());
    let before = rail(cx);
    status(cx, &view, goodbye());
    quiet(cx, &view, "the goodbye");
    // Re-gate 3: the rail keeps its last lists (no "Not connected", its
    // HISTORY block kept) while the reload is within its grace.
    assert_eq!(rail(cx), before, "the rail through a reload");
    for n in 1..=9000 / retry {
        wait(cx, w, &view, retry);
        status(cx, &view, Status::Offline("connection refused".into()));
        quiet(cx, &view, &format!("reloading, {} ms", retry * n));
    }
    assert_eq!(rail(cx), before, "the rail 9 s into a reload");
    // The words say so, at the connected words' width (NEEDS YOU, centred
    // beside them, never moves), and the disc stays lit.
    let need = |cx: &mut TestAppContext| {
        let mut vcx = gpui_kit::VisualTestContext::from_window(w, cx);
        vcx.debug_bounds("attention").map(|b| f32::from(b.origin.x))
    };
    let during = need(cx);
    status(cx, &view, hello());
    let after = need(cx);
    assert!(during.is_some() && during == after, "NEEDS YOU through a reload: {during:?} vs connected {after:?}");
    status(cx, &view, goodbye());
    status(cx, &view, hello());
    quiet(cx, &view, "back after 9 s");
    wait(cx, w, &view, 700);
    quiet(cx, &view, "settled");
    assert!(!motion_pending(cx, &view).0, "settled: no frames");
    // A reload that never comes back is a loss, its grace after the goodbye
    // (to the millisecond: motion.rs `the_link_tears_after_its_grace`).
    status(cx, &view, goodbye());
    let rg = crate::motion::RELOAD_GRACE;
    let down = until_lost(cx, w, &view);
    assert!(down >= rg && down < rg + std::time::Duration::from_millis(200), "lost {down:?} after the goodbye");
    // From here the rail and the words say so (re-gate 3).
    assert!(!view.read_with(cx, |v, _| v.model.reload), "a reload past its grace is a loss");
    frame(cx, &view);
    assert_ne!(rail(cx), before, "lost: the rail says it is not connected");
}

/// When the client is back for a moment between two frames and a second
/// goodbye comes (the window never sees it up), the new goodbye restarts
/// the grace: a slow storm is never a loss.
#[gpui_kit::test]
fn a_new_goodbye_restarts_the_reload_grace(cx: &mut TestAppContext) {
    let (w, view) = link_up(cx);
    let rg = crate::motion::RELOAD_GRACE.as_millis() as u64;
    status(cx, &view, goodbye());
    let first = view.read_with(cx, |v, _| v.link.down.map(|d| d.0)).expect("down");
    wait(cx, w, &view, rg - 2000);
    view.update(cx, |v, cx| {
        let _ = v.model.apply(synaps_client::Event::Status(hello()));
        let _ = v.model.apply(synaps_client::Event::Status(goodbye()));
        cx.notify();
    });
    cx.run_until_parked();
    let second = view.read_with(cx, |v, _| v.link.down.map(|d| d.0)).expect("down");
    assert!(second.duration_since(first).as_millis() as u64 >= rg - 2000, "the second goodbye restarted the grace");
    let down = until_lost(cx, w, &view);
    let rg = crate::motion::RELOAD_GRACE;
    assert!(down >= rg && down < rg + std::time::Duration::from_millis(200), "lost {down:?} after the second goodbye");
}

/// A daemon's availability for a reload storm: `n` reloads, each new daemon
/// taking `boot` ms to answer, the next reload as soon as it does (the
/// CLI's `daemon reload` returns once the new image answers a ping).
/// Then what the client says and when, on the reconnect schedule `wait`
/// (`after`, `now`, and whether this wait follows a goodbye).
fn storm(n: u64, boot: u64, mut wait: impl FnMut(u64, u64, bool, u64) -> u64) -> Vec<(u64, synaps_client::Status)> {
    let reloads: Vec<u64> = (0..n).map(|k| 1000 + k * (boot + 50)).collect();
    let daemon_up = |t: u64| !reloads.iter().any(|&r| r <= t && t < r + boot);
    let mut out = Vec::new();
    let (mut up_since, mut next, mut k) = (Some(0u64), 0u64, 0usize);
    loop {
        if let Some(since) = up_since {
            let Some(&r) = reloads.get(k) else { break };
            k += 1;
            out.push((r, goodbye()));
            up_since = None;
            // The goodbye's `retry_after_ms`: 500 here.
            next = r + wait(500, r, true, r - since);
            continue;
        }
        while reloads.get(k).is_some_and(|&r| r <= next) {
            k += 1;
        }
        if daemon_up(next) {
            out.push((next, hello()));
            up_since = Some(next);
        } else {
            out.push((next, synaps_client::Status::Offline("connection refused".into())));
            next += wait(0, next, false, 0);
        }
    }
    out
}

/// The longest stretch the link is down in `events`, from its last goodbye.
fn longest_from_goodbye(events: &[(u64, synaps_client::Status)]) -> u64 {
    let (mut from, mut worst) = (None, 0);
    for (t, s) in events {
        match s {
            synaps_client::Status::Reconnecting { .. } => from = Some(*t),
            synaps_client::Status::Connected { .. } => worst = worst.max(from.take().map_or(0, |f| t - f)),
            _ => {}
        }
    }
    worst
}

/// A storm of four daemon reloads as the client lives it (R1): its real
/// reconnect schedule (`synaps_client::Backoff`), each new daemon taking
/// 1.6 s to boot (bella's sandbox, empty), the CLI reloading again as soon
/// as the new one answers: about 7 s of the link down, nearly unbroken.
/// No tear, no drain, no Scan at any step. (On the old schedule, doubling
/// and never reset within 10 s, the same storm kept the link down past the
/// old 6 s grace: the sim says so first.)
#[gpui_kit::test]
fn a_four_reload_storm_on_the_clients_schedule_plays_nothing(cx: &mut TestAppContext) {
    use std::time::{Duration, Instant};
    let base = Instant::now();
    let mut b = synaps_client::Backoff::default();
    let events = storm(4, 1600, |after, t, bye, up_for| {
        let now = base + Duration::from_millis(t);
        if bye {
            b.dropped(Duration::from_millis(up_for), true, now);
        }
        b.wait(Duration::from_millis(after), now).as_millis() as u64
    });
    let mut old = 100u64;
    let was = storm(4, 1600, |after, _, _, _| {
        let w = after.max(old);
        old = (old * 2).min(5000);
        w
    });
    assert!(longest_from_goodbye(&was) > 6000, "the old schedule outlasted the old grace: {was:?}");
    let rg = crate::motion::RELOAD_GRACE.as_millis() as u64;
    assert!(longest_from_goodbye(&events) < rg / 2, "the client's schedule: {events:?}");
    let (w, view) = link_up(cx);
    let mut t = 0;
    for (at, s) in events {
        // Frames on the way, every 100 ms at most.
        while t < at {
            let d = (at - t).min(100);
            wait(cx, w, &view, d);
            t += d;
            quiet(cx, &view, &format!("{t} ms"));
        }
        let what = format!("{s:?} at {at} ms");
        status(cx, &view, s);
        quiet(cx, &view, &what);
    }
    assert!(matches!(view.read_with(cx, |v, _| v.model.conn.clone()), crate::model::ConnState::Connected(_)), "back");
    wait(cx, w, &view, 700);
    quiet(cx, &view, "settled");
    assert!(!motion_pending(cx, &view).0, "settled: no frames");
}

/// A storm of reloads (3 in 2 s), or of short drops, plays nothing; a
/// storm of real losses tears at most once per [`FLASH_BUDGET`].
#[gpui_kit::test]
fn a_reload_storm_tears_at_most_once(cx: &mut TestAppContext) {
    use crate::model::ConnState;
    let (w, view) = link_up(cx);
    let tears = std::rc::Rc::new(std::cell::Cell::new(0));
    let scans = std::rc::Rc::new(std::cell::Cell::new(0));
    let mut last = (None, None);
    let mut count = |cx: &mut TestAppContext| {
        let now = view.read_with(cx, |v, _| (v.link.tore, v.link.scanned));
        if now.0 != last.0 {
            tears.set(tears.get() + 1);
        }
        if now.1 != last.1 {
            scans.set(scans.get() + 1);
        }
        last = now;
    };
    count(cx);
    tears.set(0);
    scans.set(0);
    for _ in 0..3 {
        set_conn(cx, &view, ConnState::Reconnecting, true);
        count(cx);
        wait(cx, w, &view, 120);
        count(cx);
        set_conn(cx, &view, ConnState::Connected("x".into()), false);
        count(cx);
        wait(cx, w, &view, 540);
        count(cx);
    }
    assert_eq!((tears.get(), scans.get()), (0, 0), "a reload storm: nothing");
    // Short drops (under the grace): nothing either.
    for _ in 0..3 {
        set_conn(cx, &view, ConnState::Reconnecting, false);
        wait(cx, w, &view, 300);
        set_conn(cx, &view, ConnState::Connected("x".into()), false);
        wait(cx, w, &view, 300);
        count(cx);
    }
    assert_eq!((tears.get(), scans.get()), (0, 0), "blips: nothing");
    // Real losses, 3 in under 5 s: one tear, one Scan.
    for _ in 0..3 {
        set_conn(cx, &view, ConnState::Reconnecting, false);
        wait(cx, w, &view, crate::motion::LINK_GRACE.as_millis() as u64);
        count(cx);
        wait(cx, w, &view, 200);
        set_conn(cx, &view, ConnState::Connected("x".into()), false);
        count(cx);
        wait(cx, w, &view, 100);
        count(cx);
    }
    assert_eq!((tears.get(), scans.get()), (1, 1), "losses: at most one tear and one Scan per budget");
}

/// The drain turns from where it is (motion.md §8): back mid-tear or
/// mid-drain, then lost again (the grace outlasts the restore, so that one
/// drains from where the restore ended), the palette never moves by more
/// than the curve's own step in a frame.
#[gpui_kit::test]
fn a_blip_never_jumps_the_palette(cx: &mut TestAppContext) {
    use crate::model::ConnState;
    let drain = |cx: &mut TestAppContext| cx.update(|cx| cx.global::<crate::theme::Scheme>().drain.get());
    // `inout`'s steepest slope is 2.86× linear: a 7 ms frame's largest step.
    let max_step = 7. / crate::motion::SLOW.as_millis() as f32 * 3.;
    let grace = crate::motion::LINK_GRACE.as_millis() as u64;
    for back_at in [50u64, 150, 300] {
        let (w, view) = link_up(cx);
        let prev = std::cell::Cell::new(drain(cx));
        let frames = |cx: &mut TestAppContext, ms: u64, what: &str| {
            for _ in 0..ms.div_ceil(7) {
                wait(cx, w, &view, 7);
                let d = drain(cx);
                assert!((d - prev.get()).abs() <= max_step, "{what}, back at {back_at} ms: {} → {d}", prev.get());
                prev.set(d);
            }
        };
        set_conn(cx, &view, ConnState::Reconnecting, false);
        frames(cx, grace + back_at, "down");
        assert!(prev.get() > 0. || back_at < 100, "lost: draining ({})", prev.get());
        set_conn(cx, &view, ConnState::Connected("x".into()), false);
        frames(cx, 140, "back");
        let mid = prev.get();
        set_conn(cx, &view, ConnState::Reconnecting, false);
        frames(cx, grace + 200, "lost again");
        set_conn(cx, &view, ConnState::Connected("x".into()), false);
        frames(cx, 500, "back again");
        assert_eq!(prev.get(), 0., "restored (mid {mid})");
    }
}

/// The tracer starts from the source chip's bar (motion.md §6.2: its left
/// edge, at its centre) when the pointer was on the chip; elsewhere, from
/// the pointer.
#[gpui_kit::test]
fn tracer_starts_from_the_chips_bar(cx: &mut TestAppContext) {
    use gpui_kit::{MouseMoveEvent, PlatformInput, point};
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    settle(cx, w, 4);
    let scout = view.read_with(cx, |v, _| {
        let r = v.tray.rows.iter().find(|r| r.id == 1).expect("scout's tray row");
        crate::wtabs::Who::new(1, r.name.clone(), r.call.clone())
    });
    let chip =
        view.read_with(cx, |v, _| v.chip_bounds.borrow().iter().find(|(id, _)| *id == 1).map(|(_, b)| *b)).expect("its chip laid out");
    let to = |cx: &mut TestAppContext, at: gpui_kit::Point<gpui_kit::Pixels>| {
        event(cx, w, PlatformInput::MouseMove(MouseMoveEvent { position: at, pressed_button: None, modifiers: Default::default() }));
        view.update(cx, |v, cx| v.open_worker_tab(&scout, cx));
        cx.run_until_parked();
        wait(cx, w, &view, 7);
        let from = view.read_with(cx, |v, _| v.tracer.map(|t| t.from)).expect("a tracer");
        // Its tab closed: the next open is a new tab again.
        view.update(cx, |v, cx| {
            v.tracer = None;
            let ix = v.model.tabs.iter().position(|t| t.worker.is_some()).expect("the worker's tab");
            v.close_tab(ix);
            cx.notify();
        });
        settle(cx, w, 4);
        from
    };
    let on_chip = point(chip.left() + chip.size.width * 0.75, chip.center().y);
    assert_eq!(to(cx, on_chip), point(chip.left(), chip.center().y), "from the chip's bar");
    let off = point(chip.left(), chip.top() - gpui_kit::px(40.));
    assert_eq!(to(cx, off), off, "elsewhere, from the pointer");
}

/// A worker tab opened by the pointer draws the tracer to it (motion.md
/// §6.2): the line lands on the new tab's top-left corner over `base`, the
/// tab's bar draws over `quick`, then it is gone. Reduced: none.
#[gpui_kit::test]
fn tracer_lands_on_new_tab(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    settle(cx, w, 4);
    let scout = view.read_with(cx, |v, _| {
        let r = v.tray.rows.iter().find(|r| r.id == 1).expect("scout's tray row");
        crate::wtabs::Who::new(1, r.name.clone(), r.call.clone())
    });
    view.update(cx, |v, cx| v.open_worker_tab(&scout, cx));
    cx.run_until_parked();
    wait(cx, w, &view, 7);
    let (tr, tab) = view.read_with(cx, |v, _| (v.tracer, v.model.tabs.current().map(|t| t.id)));
    let tr = tr.expect("a tracer");
    assert_eq!(Some(tr.tab), tab, "to the new tab");
    let to = tr.to.expect("its target laid out");
    let strip = view.read_with(cx, |v, _| v.strip_at.get());
    assert!(strip.y > px(0.) && to.y == strip.y, "the tab's top edge: {to:?} {strip:?}");
    // `base` + `quick`: 320 ms, 47 frames at 144 Hz.
    for _ in 0..30 {
        wait(cx, w, &view, 7);
    }
    assert!(view.read_with(cx, |v, _| v.tracer.is_some()), "the bar drawing");
    for _ in 0..20 {
        wait(cx, w, &view, 7);
    }
    let left = view.read_with(cx, |v, _| v.tracer.map(|t| (t, v.fx.clock().duration_since(t.at))));
    assert!(left.is_none(), "landed, the bar drawn: gone ({left:?})");
    // Reduced: none.
    view.update(cx, |v, cx| v.leave_worker(false, cx));
    set_motion(cx, &view, crate::motion::Level::Reduced);
    settle(cx, w, 4);
    view.update(cx, |v, cx| v.open_worker_tab(&scout, cx));
    cx.run_until_parked();
    settle(cx, w, 4);
    assert!(view.read_with(cx, |v, _| v.tracer.is_none()), "Reduced: no tracer");
}

/// The palette's keys as this OS spells them (Ctrl+P / Ctrl+K, Cmd on macOS).
fn palette_key(commands: bool) -> &'static str {
    let (goto, cmds) = crate::app::palette_keys(cfg!(target_os = "macos"));
    if commands { cmds } else { goto }
}

/// `text` typed into what has the focus, as the platform's text input.
fn type_in(cx: &mut TestAppContext, w: AnyWindowHandle, text: &str) {
    VisualTestContext::from_window(w, cx).simulate_input(text);
    cx.run_until_parked();
}

fn palette_mode(cx: &mut TestAppContext, view: &Entity<AppView>) -> Option<crate::palette::Mode> {
    view.read_with(cx, |v, _| v.palette.as_ref().map(|p| p.mode))
}

fn palette_focused(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) -> bool {
    cx.update_window(w, |_, window, cx| view.read(cx).palette_input.read(cx).focus_handle(cx).is_focused(window)).expect("the window")
}

/// polish.md P16: Ctrl+P opens GO TO from the composer (its input binds
/// neither key), the query focused and the card's keys off while it is
/// open; Esc closes it and gives the composer its focus back. That Esc is
/// the palette's: it arms no HALT and denies nothing.
#[gpui_kit::test]
fn palette_esc_restores_focus(cx: &mut TestAppContext) {
    use crate::palette::Mode;
    let (w, view, _events) = open(cx);
    resolve_prompt(cx, &view);
    focus_composer(cx, w, &view);
    tap(cx, w, palette_key(false));
    assert_eq!(palette_mode(cx, &view), Some(Mode::GoTo));
    assert!(palette_focused(cx, w, &view) && !composer_focused(cx, w, &view), "the query has the focus");
    tap(cx, w, "escape");
    assert_eq!(palette_mode(cx, &view), None, "Esc closes it");
    assert!(composer_focused(cx, w, &view), "the focus is back in the composer");
    assert!(view.read_with(cx, |v, _| v.halts == 0 && v.halt_armed.is_none()), "the palette's Esc arms no HALT");
    // From the window's own focus (the card up): back there, and the card's
    // keys are off while it is open.
    let (w, view, _events) = open(cx);
    card_keys(cx, w, &view);
    tap(cx, w, palette_key(true));
    assert_eq!(palette_mode(cx, &view), Some(Mode::Commands));
    settle(cx, w, 4);
    assert_eq!(view.read_with(cx, |v, _| v.card_keys.get()), None, "the card's keys are off under the palette");
    tap(cx, w, "escape");
    assert_eq!(palette_mode(cx, &view), None);
    assert!(view.read_with(cx, |v, _| v.current_card().is_some()), "Esc closed the palette, not the card (no DENY)");
    assert!(cx.update_window(w, |_, window, cx| view.read(cx).focus.is_focused(window)).expect("the window"), "the window's focus again");
}

/// polish.md P16: Tab belongs to the palette (never typed, never the kit's
/// focus cycling, never the card's Allow all): it flips the mode; the other
/// mode's key switches to it, its own key leaves it.
#[gpui_kit::test]
fn palette_tab_flips_mode(cx: &mut TestAppContext) {
    use crate::palette::Mode;
    let (w, view, _events) = open(cx);
    focus_composer(cx, w, &view);
    tap(cx, w, palette_key(false));
    tap(cx, w, "tab");
    assert_eq!(palette_mode(cx, &view), Some(Mode::Commands), "Tab: commands");
    tap(cx, w, "shift-tab");
    assert_eq!(palette_mode(cx, &view), Some(Mode::GoTo), "Shift+Tab: back");
    assert!(palette_focused(cx, w, &view), "the query keeps the focus");
    assert_eq!(view.read_with(cx, |v, cx| v.palette_input.read(cx).value().to_string()), "", "no tab typed");
    tap(cx, w, palette_key(true));
    assert_eq!(palette_mode(cx, &view), Some(Mode::Commands), "Ctrl+K switches");
    tap(cx, w, palette_key(true));
    assert_eq!(palette_mode(cx, &view), Some(Mode::Commands), "and stays");
    tap(cx, w, palette_key(false));
    assert_eq!(palette_mode(cx, &view), Some(Mode::GoTo), "Ctrl+P switches back");
    // ↓ then Enter picks; the palette closes.
    let first = view.read_with(cx, |v, _| v.palette.as_ref().and_then(|p| p.selected().map(|e| e.group)));
    assert_eq!(first, Some(crate::palette::Group::Sessions), "GO TO starts at the sessions");
    tap(cx, w, "enter");
    assert_eq!(palette_mode(cx, &view), None, "Enter picks and closes");
}

/// polish.md P16 / fluid.md §3.10: opened by key, the palette is whole in
/// its first frame (its list's height and the selection quad at their
/// targets); then the quad springs to a new row and the height to what the
/// query leaves, on `snap`. Reduced and Off: at once.
#[gpui_kit::test]
fn palette_keyboard_open_is_instant(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    focus_composer(cx, w, &view);
    tap(cx, w, palette_key(true));
    let drawn = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.palette_drawn.get());
    let (h, ht, q, qt) = drawn(cx);
    assert!(ht > 0. && h == ht && q == qt, "whole in its first frame: {h} / {ht}, quad {q} / {qt}");
    tap(cx, w, "down");
    let (_, _, q, qt) = drawn(cx);
    assert!(q != qt, "the quad springs to the next row: {q} → {qt}");
    for _ in 0..60 {
        wait(cx, w, &view, 7);
    }
    let (_, _, q, qt) = drawn(cx);
    assert_eq!(q, qt, "and lands");
    type_in(cx, w, "/model");
    frame(cx, &view);
    let (h, ht, _, _) = drawn(cx);
    assert!(h != ht && ht < 100., "the height springs to what is left: {h} → {ht}");
    for _ in 0..60 {
        wait(cx, w, &view, 7);
    }
    let (h, ht, _, _) = drawn(cx);
    assert_eq!(h, ht, "and settles");
    for level in [crate::motion::Level::Reduced, crate::motion::Level::Off] {
        set_motion(cx, &view, level);
        tap(cx, w, "escape");
        tap(cx, w, palette_key(true));
        tap(cx, w, "down");
        let (h, ht, q, qt) = drawn(cx);
        assert!(h == ht && q == qt, "{level:?}: at once");
    }
}

/// polish.md P16: an engine command is the session owner's: observing,
/// the palette marks it and picking it sends nothing, politely; driving,
/// it goes out as the engine command, and the composer's `/compact` takes
/// the same path (never sent as a message).
#[gpui_kit::test]
fn palette_engine_commands_are_the_owners(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    resolve_prompt(cx, &view);
    let mode = view.read_with(cx, |v, _| v.model.active.as_ref().unwrap().mode);
    view.update(cx, |v, cx| {
        v.model.active.as_mut().unwrap().mode = synaps_client::AttachMode::Observe;
        cx.notify();
    });
    cx.run_until_parked();
    focus_composer(cx, w, &view);
    tap(cx, w, palette_key(true));
    type_in(cx, w, "/compact");
    let sel = view.read_with(cx, |v, _| v.palette.as_ref().and_then(|p| p.selected().cloned())).expect("a row");
    assert_eq!(sel.act, crate::palette::Act::Engine("compact"));
    assert_eq!(sel.key.as_deref(), Some("owner"), "marked");
    assert_eq!(sel.off.as_deref(), Some("/compact: this window observes"), "{:?}", sel.off);
    tap(cx, w, "enter");
    let note = view.read_with(cx, |v, _| v.watch_note.clone()).expect("a note");
    assert_eq!(note.as_ref(), "/compact: this window observes", "refused politely");
    // Driving: it goes out (the client takes it; its daemon is nowhere).
    view.update(cx, |v, cx| {
        v.model.active.as_mut().unwrap().mode = mode;
        v.watch_note = None;
        cx.notify();
    });
    cx.run_until_parked();
    // (The demo's turn over: mid-turn neither door runs a command.)
    idle(cx, &view);
    assert!(view.update(cx, |v, _| v.run_command("context", String::new())), "sent");
    assert!(view.read_with(cx, |v, _| v.watch_note.clone()).is_some_and(|n| n.contains("/context: sent")));
    // The composer's `/compact`: the same path, not a message.
    cx.update_window(w, |_, window, cx| {
        view.update(cx, |v, cx| {
            v.composer.update(cx, |s, cx| {
                s.set_value("/compact", window, cx);
                s.focus(window, cx);
            })
        })
    })
    .expect("the window");
    cx.run_until_parked();
    let sent_before = view.read_with(cx, |v, _| v.model.transcript.items().len());
    tap(cx, w, "enter");
    assert!(view.read_with(cx, |v, _| v.watch_note.clone()).is_some_and(|n| n.contains("/compact: sent")), "the engine's");
    assert_eq!(view.read_with(cx, |v, _| v.model.transcript.items().len()), sent_before, "no message in the log");
    assert_eq!(composer_text(cx, &view), "/compact", "kept until the daemon has run it");
}

/// The session's turn over (the demo's runs): no turn running.
fn idle(cx: &mut TestAppContext, view: &Entity<AppView>) {
    view.update(cx, |v, cx| {
        v.model.transcript.set_busy(false);
        cx.notify();
    });
    cx.run_until_parked();
    assert!(!view.read_with(cx, |v, _| v.model.transcript.busy()));
}

/// `text` in the composer, focused, then Enter.
fn send_in_composer(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, text: &str) {
    cx.update_window(w, |_, window, cx| {
        view.update(cx, |v, cx| {
            v.watch_note = None;
            v.composer.update(cx, |s, cx| {
                s.set_value(text, window, cx);
                s.focus(window, cx);
            })
        })
    })
    .expect("the window");
    cx.run_until_parked();
    tap(cx, w, "enter");
}

/// The daemon's answer to slash command `name` on the attachment on screen.
fn reply(cx: &mut TestAppContext, view: &Entity<AppView>, name: &str, reply: synaps_client::CommandReply) {
    view.update(cx, |v, cx| {
        let attach_seq = v.model.active.as_ref().expect("attached").attach_seq;
        v.apply_batch(vec![synaps_client::Event::EngineReply { attach_seq, name: name.into(), reply }], cx);
    });
    cx.run_until_parked();
}

fn in_flight(cx: &mut TestAppContext, view: &Entity<AppView>) -> Vec<String> {
    view.read_with(cx, |v, _| v.model.in_flight.iter().cloned().collect())
}

/// Shady #20 H2: the composer's text is an engine command only when the
/// whole of it is a valid invocation; prose that starts with a command
/// word goes as a message, exactly as before. A command's text stays in
/// the composer until the daemon has run it (a `/model x` that left
/// another model in use has not); a run one leaves.
#[gpui_kit::test]
fn the_composer_runs_only_whole_valid_commands(cx: &mut TestAppContext) {
    use synaps_client::CommandReply::{Done, NotRun};
    let (w, view, _events) = open(cx);
    resolve_prompt(cx, &view);
    idle(cx, &view);
    for prose in ["/model is this the wrong one?", "/workers are stuck", "/usr/bin"] {
        send_in_composer(cx, w, &view, prose);
        assert_eq!(in_flight(cx, &view).last().map(String::as_str), Some(prose), "{prose}: a message");
        assert_eq!(composer_text(cx, &view), "", "{prose}: sent, as before");
        assert!(view.read_with(cx, |v, _| v.watch_note.is_none()), "{prose}: no command's note");
    }
    let messages = in_flight(cx, &view).len();
    // A valid one: the engine's, kept until it has run.
    send_in_composer(cx, w, &view, "/compact");
    assert_eq!(in_flight(cx, &view).len(), messages, "not a message");
    assert!(view.read_with(cx, |v, _| v.watch_note.clone()).is_some_and(|n| n.contains("/compact: sent")));
    assert_eq!(composer_text(cx, &view), "/compact");
    reply(cx, &view, "compact", Done(String::new()));
    assert!(view.read_with(cx, |v, _| v.watch_note.clone()).is_some_and(|n| n.contains("/compact: asked")), "none is asked");
    frame(cx, &view);
    assert_eq!(composer_text(cx, &view), "", "run: it leaves the composer");
    // `/model x` the daemon did not switch to: not run, the text stays.
    send_in_composer(cx, w, &view, "/model openai/gpt-5.1");
    assert_eq!(in_flight(cx, &view).len(), messages);
    reply(cx, &view, "model", NotRun("not switched: the model is still claude-opus-4-6".into()));
    frame(cx, &view);
    let note = view.read_with(cx, |v, _| v.watch_note.clone()).expect("a note");
    assert!(note.contains("/model not run: not switched"), "{note}");
    assert_eq!(composer_text(cx, &view), "/model openai/gpt-5.1", "not run: the text stays");
    // Its whole answer is a row of the transcript's (this client's).
    send_in_composer(cx, w, &view, "/workers");
    let rows: String = (1..=12).map(|k| format!("\n  sa_{k}  scout  running  0m")).collect();
    reply(cx, &view, "workers", Done(format!("Workers (12):{rows}")));
    frame(cx, &view);
    let last = view.read_with(cx, |v, _| v.model.transcript.items().last().map(|i| (i.kind, i.text.clone(), i.is_note())));
    let (kind, text, note) = last.expect("a row");
    assert_eq!(kind, synaps_client::transcript::ItemKind::Output);
    assert!(note && text.starts_with("/workers\nWorkers (12):") && text.ends_with("sa_12  scout  running  0m"), "{text}");
    assert_eq!(in_flight(cx, &view).len(), messages, "never sent");
    assert!(view.read_with(cx, |v, _| v.watch_note.clone()).is_some_and(|n| n.contains("/workers: 13 lines, in the transcript")));
}

/// Shady #20 H2: while a turn runs, prose that starts with a command word
/// steers, as it always did; a valid command does what the TUI's does
/// there: it is not run, nothing is sent, and the text stays.
#[gpui_kit::test]
fn mid_turn_a_command_is_not_run_and_prose_steers(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    resolve_prompt(cx, &view);
    start_stream(cx, &view);
    assert!(view.read_with(cx, |v, _| v.model.transcript.busy()));
    let before = in_flight(cx, &view).len();
    for t in ["/compact please", "/workers"] {
        send_in_composer(cx, w, &view, t);
        assert_eq!(in_flight(cx, &view).len(), before, "{t}: nothing sent");
        assert_eq!(composer_text(cx, &view), t, "{t}: the text stays");
        let note = view.read_with(cx, |v, _| v.watch_note.clone()).expect("a note");
        assert!(note.contains("not while a turn runs"), "{note}");
    }
    send_in_composer(cx, w, &view, "/workers are stuck, look");
    assert_eq!(in_flight(cx, &view).last().map(String::as_str), Some("/workers are stuck, look"), "a steer");
    assert_eq!(composer_text(cx, &view), "");
}

/// Shady #20 M1: "Favourite this session" picked in a palette opened over
/// the composer (the focus goes back there first) favourites it.
#[gpui_kit::test]
fn palette_favourite_from_the_composer(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    resolve_prompt(cx, &view);
    let id = attached(cx, &view).expect("a session");
    assert!(view.read_with(cx, |v, _| v.model.favs.position(&id).is_none()));
    focus_composer(cx, w, &view);
    tap(cx, w, palette_key(true));
    type_in(cx, w, "favourite");
    tap(cx, w, "enter");
    assert_eq!(palette_mode(cx, &view), None, "picked");
    assert!(composer_focused(cx, w, &view), "the focus is back in the composer");
    assert!(view.read_with(cx, |v, _| v.model.favs.position(&id).is_some()), "and the session is a favourite");
}

/// Shady #20 M3: Fold all / Unfold all keep the row at the top (and how far
/// into it) with the view between runs, not the first run that changed.
#[gpui_kit::test]
fn fold_all_keeps_the_top_row(cx: &mut TestAppContext) {
    use crate::turns::Kind;
    let (w, view, _events) = open(cx);
    view.update(cx, |v, _| v.client = None);
    open_idle_cpu(cx, &view, crate::tabs::Place::Current);
    settle(cx, w, 8);
    view.update(cx, |v, cx| v.fold_all_runs(true, cx));
    settle(cx, w, 8);
    // The row after the first run's last: between it and the next.
    let (ix, id) = view.read_with(cx, |v, _| {
        let first = v.disp.iter().position(|d| matches!(d.kind, Kind::Summary { .. })).expect("a run");
        let Kind::Summary { end, .. } = v.disp[first].kind else { unreachable!() };
        let ix = v.disp.iter().position(|d| d.item >= end).expect("a row after it");
        assert!(v.disp[ix + 1..].iter().any(|d| matches!(d.kind, Kind::Summary { .. })), "a run below it too");
        (ix, v.disp[ix].id)
    });
    let top = |cx: &mut TestAppContext| {
        view.read_with(cx, |v, _| {
            let t = v.list.logical_scroll_top();
            (v.disp[t.item_ix].id, t.offset_in_item)
        })
    };
    view.update(cx, |v, cx| {
        v.list.pause_following_tail();
        v.list.scroll_to(gpui_kit::ListOffset { item_ix: ix, offset_in_item: px(5.) });
        cx.notify();
    });
    settle(cx, w, 8);
    assert_eq!(top(cx), (id, px(5.)));
    assert!(!view.read_with(cx, |v, _| v.list.is_following_tail()));
    for open in [false, true, false] {
        view.update(cx, |v, cx| v.fold_all_runs(open, cx));
        settle(cx, w, 8);
        assert_eq!(top(cx), (id, px(5.)), "fold all ({open}): the same row at the top");
    }
}

/// Design gate #20 (1): the box hangs under the tab strip (never over a
/// tab's name) and ends 12 px or more above the composer's frame, at
/// 1280×720 and 1920×1080.
fn palette_fits(cx: &mut TestAppContext, (wd, ht): (f32, f32)) {
    let (w, view, _events) = open_at(cx, (wd, ht), crate::theme::Panels::default().text_size);
    focus_composer(cx, w, &view);
    tap(cx, w, palette_key(false));
    frame(cx, &view);
    let (top, bottom, ..) = view.read_with(cx, |v, _| v.palette_box.get());
    let strip = view.read_with(cx, |v, _| f32::from(v.strip_at.get().y)) + crate::tabstrip::TAB_H;
    let composer = view.read_with(cx, |v, _| v.composer_top.get());
    assert!(strip > crate::tabstrip::TAB_H && composer > top, "laid out: strip {strip}, composer {composer}");
    assert!(top >= strip + 8., "{wd}: the box's top {top} under the strip's bottom {strip}");
    assert!(bottom <= composer - 12., "{wd}: the box's bottom {bottom}, the composer's top {composer}");
    // Design re-gate #20 (1): the demo's worker chips show over the
    // composer: the box ends 12 px or more above them too.
    let chips = view.read_with(cx, |v, _| v.chip_bounds.borrow().iter().map(|c| f32::from(c.1.origin.y)).reduce(f32::min));
    let chips = chips.expect("the demo's chips show");
    assert!(chips < composer, "the chips over the composer: {chips}, {composer}");
    assert!(bottom <= chips - 12., "{wd}: the box's bottom {bottom}, the chips' top {chips}");
}

/// Design re-gate #20 (3): an answered command's note is quiet (`dim_u`),
/// a refusal a warning; `/compact`'s "asked" note goes once the compaction
/// it asked for has its notice in the transcript; and a note cut short has
/// the house tooltip.
#[gpui_kit::test]
fn command_notes_are_quiet_and_never_stale(cx: &mut TestAppContext) {
    use synaps_client::CommandReply::Done;
    let (w, view, _events) = open(cx);
    resolve_prompt(cx, &view);
    idle(cx, &view);
    let color = |cx: &mut TestAppContext| {
        view.read_with(cx, |v, cx| {
            let p = crate::theme::palette(cx);
            let n = v.watch_note.clone().expect("a note");
            (v.note_color(&n, &p) == p.dim_u, v.note_color(&n, &p) == p.warn)
        })
    };
    send_in_composer(cx, w, &view, "/compact");
    assert_eq!(color(cx), (true, false), "sent: quiet");
    reply(cx, &view, "compact", Done(String::new()));
    assert_eq!(note(cx, &view), "/compact: asked · see the transcript");
    assert_eq!(color(cx), (true, false), "answered: quiet");
    assert!(VisualTestContext::from_window(w, cx).debug_bounds("composer-note").is_some(), "drawn in the foot");
    view.update(cx, |v, cx| {
        let seq = v.model.active.as_ref().expect("attached").attach_seq;
        let sid = v.model.active.as_ref().expect("attached").session_id.clone();
        let ev = synaps_client::protocol::SessionEvent::CompactionApplied {
            previous_session_id: sid.clone(),
            session_id: sid.clone(),
            msg_count: 2,
        };
        v.apply_batch(vec![synaps_client::Event::session(seq, sid, 1, ev, std::sync::Arc::new(serde_json::Value::Null))], cx);
    });
    cx.run_until_parked();
    assert_eq!(note(cx, &view), "", "the compaction's notice is in the transcript: the note is done");
    // Refused (mid-turn): a warning.
    start_stream(cx, &view);
    send_in_composer(cx, w, &view, "/budget");
    assert_eq!(color(cx), (false, true), "refused: a warning");
}

/// Design re-gate #20 (5), found in its clip: reopened after scrolling
/// down, the list is at its top again, its first group's head in view (the
/// quad is the list's first child: scrolled to, it hid the head).
#[gpui_kit::test]
fn palette_reopens_at_the_top(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    focus_composer(cx, w, &view);
    tap(cx, w, palette_key(false));
    for _ in 0..12 {
        tap(cx, w, "down");
    }
    tap(cx, w, "pagedown");
    settle(cx, w, 8);
    let y = |cx: &mut TestAppContext| view.read_with(cx, |v, _| f32::from(v.palette_scroll.offset().y));
    assert!(y(cx) < 0., "scrolled down");
    tap(cx, w, "escape");
    tap(cx, w, palette_key(false));
    settle(cx, w, 8);
    assert_eq!(y(cx), 0., "at the top: the head in view");
    type_in(cx, w, "e");
    settle(cx, w, 8);
    assert_eq!(y(cx), 0., "a query: the top too");
}

/// Design re-gate #20 (2): a query that matches nothing shows "Nothing
/// matches": the list is a row high, never 0.
#[gpui_kit::test]
fn palette_miss_shows_nothing_matches(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    focus_composer(cx, w, &view);
    tap(cx, w, palette_key(true));
    type_in(cx, w, "zzqxv");
    for _ in 0..20 {
        wait(cx, w, &view, 33);
    }
    let (h, target, _, _) = view.read_with(cx, |v, _| v.palette_drawn.get());
    assert_eq!((h, target), (crate::palette::ROW_H, crate::palette::ROW_H), "a row high");
    assert!(VisualTestContext::from_window(w, cx).debug_bounds("palette-empty").is_some_and(|b| b.size.height > px(0.)), "drawn");
}

#[gpui_kit::test]
fn palette_fits_at_1280(cx: &mut TestAppContext) {
    palette_fits(cx, (1280., 720.));
}

#[gpui_kit::test]
fn palette_fits_at_1920(cx: &mut TestAppContext) {
    palette_fits(cx, (1920., 1080.));
}

/// Shady #20 L5: under the open palette the app's keys do nothing (the
/// tabs, the panels); Ctrl+P / Ctrl+K still act.
#[gpui_kit::test]
fn app_keys_are_off_under_the_palette(cx: &mut TestAppContext) {
    use crate::palette::Mode;
    let (w, view, _events) = open(cx);
    tap(cx, w, palette_key(false));
    let tabs = |cx: &mut TestAppContext| view.read_with(cx, |v, _| (v.model.tabs.iter().count(), v.panels));
    let before = tabs(cx);
    let (rail, _) = crate::app::panel_keys(cfg!(target_os = "macos"));
    let new_tab = if cfg!(target_os = "macos") { "cmd-t" } else { "ctrl-t" };
    for k in [new_tab, rail] {
        tap(cx, w, k);
        assert_eq!(tabs(cx), before, "{k}: nothing under the palette");
        assert_eq!(palette_mode(cx, &view), Some(Mode::GoTo), "{k}: it stays open");
    }
    tap(cx, w, palette_key(true));
    assert_eq!(palette_mode(cx, &view), Some(Mode::Commands));
    tap(cx, w, "escape");
    tap(cx, w, rail);
    assert_ne!(tabs(cx), before, "closed, the key acts");
}

/// The palette's selected row.
fn palette_sel(cx: &mut TestAppContext, view: &Entity<AppView>) -> crate::palette::Entry {
    view.read_with(cx, |v, _| v.palette.as_ref().and_then(|p| p.selected().cloned())).expect("a row")
}

fn note(cx: &mut TestAppContext, view: &Entity<AppView>) -> String {
    view.read_with(cx, |v, _| v.watch_note.as_ref().map(|n| n.to_string())).unwrap_or_default()
}

/// Shady #20 N1: the palette's engine commands keep the composer's mid-turn
/// rule (one rule, in `run_command`): while a turn runs their rows are dim
/// and say why; picked, they are not run and say what the composer says.
#[gpui_kit::test]
fn mid_turn_the_palette_runs_no_command(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    resolve_prompt(cx, &view);
    start_stream(cx, &view);
    focus_composer(cx, w, &view);
    tap(cx, w, palette_key(true));
    type_in(cx, w, "/workers");
    let sel = palette_sel(cx, &view);
    assert_eq!(sel.act, crate::palette::Act::Engine("workers"));
    assert_eq!(sel.meta.to_string(), "not while a turn runs", "the row says why");
    assert_eq!(sel.off.as_deref(), Some(crate::palette::mid_turn("workers").as_str()), "dim");
    tap(cx, w, "enter");
    assert_eq!(palette_mode(cx, &view), None);
    assert_eq!(note(cx, &view), crate::palette::mid_turn("workers"), "the composer's note");
    assert!(!view.update(cx, |v, _| v.run_command("budget", String::new())), "the door itself: not run");
    assert_eq!(note(cx, &view), crate::palette::mid_turn("budget"));
    // The turn over: the rows as ever, and picked, it goes out.
    idle(cx, &view);
    tap(cx, w, palette_key(true));
    type_in(cx, w, "/workers");
    let sel = palette_sel(cx, &view);
    assert_eq!((sel.off.as_deref(), sel.meta.to_string().as_str()), (None, "the session's workers"));
    tap(cx, w, "enter");
    assert!(note(cx, &view).contains("/workers: sent"), "{}", note(cx, &view));
}

/// Shady #20 N3: a command's own words its grammar does not take are a
/// command mistyped, not chat for the model: its usage, the text kept,
/// nothing sent. N2 (the TUI's escape): `//text` sends `/text` as a
/// message (a steer mid-turn), never as a command.
#[gpui_kit::test]
fn a_mistyped_command_shows_its_usage_and_double_slash_sends_text(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    resolve_prompt(cx, &view);
    idle(cx, &view);
    let before = in_flight(cx, &view).len();
    for (t, name) in [
        ("/model", "model"),
        ("/model sonnet", "model"),
        ("/workers cancel scout", "workers"),
        ("/budget time 30", "budget"),
        ("/context auto 1", "context"),
        ("/trace next please", "trace"),
        ("/confirms reset all", "confirms"),
    ] {
        send_in_composer(cx, w, &view, t);
        assert_eq!(in_flight(cx, &view).len(), before, "{t}: nothing sent");
        assert_eq!(composer_text(cx, &view), t, "{t}: the text stays");
        assert_eq!(note(cx, &view), crate::palette::usage(name), "{t}");
    }
    send_in_composer(cx, w, &view, "//compact is broken, why?");
    assert_eq!(in_flight(cx, &view).last().map(String::as_str), Some("/compact is broken, why?"), "a message");
    assert_eq!(composer_text(cx, &view), "", "sent");
    assert_eq!(note(cx, &view), "", "no command's note");
    start_stream(cx, &view);
    send_in_composer(cx, w, &view, "//workers");
    assert_eq!(in_flight(cx, &view).last().map(String::as_str), Some("/workers"), "a steer");
    assert_eq!(composer_text(cx, &view), "");
}

/// Shady #20 N8: a tab left while its slash command was out: the answer
/// goes to an attachment no longer on screen, so the command's text leaves
/// with it (as a sent message's does): shown again, the tab's draft does
/// not hold a command already run. An edited one is a draft: kept.
#[gpui_kit::test]
fn a_tab_left_mid_command_drops_the_command_from_its_draft(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    resolve_prompt(cx, &view);
    idle(cx, &view);
    send_in_composer(cx, w, &view, "/compact");
    assert_eq!(composer_text(cx, &view), "/compact", "out: kept until it has run");
    let client = view.update(cx, |v, _| v.client.take());
    open_idle_cpu(cx, &view, crate::tabs::Place::New);
    frame(cx, &view);
    assert_eq!(composer_text(cx, &view), "", "the other tab's (empty) draft");
    // There, a command out, then edited: a draft.
    view.update(cx, |v, _| v.client = client);
    send_in_composer(cx, w, &view, "/budget");
    assert!(note(cx, &view).contains("/budget: sent"), "{}", note(cx, &view));
    cx.update_window(w, |_, window, cx| view.update(cx, |v, cx| v.composer.update(cx, |s, cx| s.set_value("/budget status", window, cx))))
        .expect("the window");
    let back = |cx: &mut TestAppContext, ix: usize| {
        view.update(cx, |v, cx| {
            v.focus_tab(ix);
            cx.notify();
        });
        cx.run_until_parked();
        frame(cx, &view);
    };
    back(cx, 0);
    assert_eq!(composer_text(cx, &view), "", "its command left with its answer");
    back(cx, 1);
    assert_eq!(composer_text(cx, &view), "/budget status", "an edited draft stays");
}

/// Shady #20 N4: a command's answer is synaps's words, not the agent's: its
/// row has its own label, and to the sticky labels it is a message of its
/// own: while it is the top message, the label pinned is its own, never
/// the agent's above it.
#[gpui_kit::test]
fn a_command_answer_pins_its_own_label(cx: &mut TestAppContext) {
    use crate::turns::Who;
    let (w, view, _events) = sticky_open(cx, (2200., 640.));
    let body: String = (1..=6).map(|k| format!("\n  line {k}")).collect();
    view.update(cx, |v, cx| {
        v.model.transcript.output("budget", &format!("Budget:{body}"));
        cx.notify();
    });
    start_stream(cx, &view);
    for k in 0..80 {
        stream_step(cx, &view, k);
    }
    idle(cx, &view);
    settle(cx, w, 8);
    let (out, id, agent) = view.read_with(cx, |v, _| {
        let out = v.disp.iter().position(|d| d.who == Who::Synaps).expect("the answer's row, labelled");
        let agent = (0..out).rev().find(|&i| v.disp[i].who == Who::Agent).expect("the agent's message above");
        (out, v.disp[out].id, v.disp[agent].id)
    });
    let pad_y = view.read_with(cx, |_, cx| crate::theme::metrics(cx).pad_y);
    scroll_list(cx, w, &view, out, 0.);
    let (_, top) = viewport(cx, &view);
    let slot = drawn(cx, w, format!("hang-label-{id}")).expect("its label in its slot");
    let off = slot[1] - top - pad_y;
    assert!(off > 0., "{slot:?}");
    // Its label past the viewport's top (the agent's, pushed by it, gone).
    scroll_list(cx, w, &view, out, off + pad_y + 2.);
    let stuck = drawn(cx, w, format!("sticky {id}")).expect("its own label pinned");
    assert!((stuck[1] - (top + pad_y)).abs() < 1e-3, "at the pin: {stuck:?}");
    assert!(drawn(cx, w, format!("sticky {agent}")).is_none(), "never the agent's");
}

// ── sticky labels (polish.md P15) ─────────────────────────────────────

/// The other demo session (three finished turns, its history) at `size`,
/// under Off, at rest.
fn sticky_open(
    cx: &mut TestAppContext,
    size: (f32, f32),
) -> (AnyWindowHandle, Entity<AppView>, async_channel::Receiver<synaps_client::Event>) {
    let (w, view, events) = open_at(cx, size, crate::theme::Panels::default().text_size);
    set_motion(cx, &view, crate::motion::Level::Off);
    view.update(cx, |v, _| v.client = None);
    open_idle_cpu(cx, &view, crate::tabs::Place::Current);
    settle(cx, w, 8);
    (w, view, events)
}

/// The list's top row `ix`, `off` px into it, drawn.
fn scroll_list(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, ix: usize, off: f32) {
    view.update(cx, |v, cx| {
        v.list.scroll_to(gpui_kit::ListOffset { item_ix: ix, offset_in_item: px(off) });
        cx.notify();
    });
    cx.run_until_parked();
    settle(cx, w, 4);
    assert_eq!(view.read_with(cx, |v, _| v.list.logical_scroll_top().item_ix), ix, "row {ix} on top");
}

/// The list scrolled by `dy` px (down the transcript), drawn.
fn scroll_list_by(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, dy: f32) {
    view.update(cx, |v, cx| {
        v.list.scroll_by(px(dy));
        cx.notify();
    });
    cx.run_until_parked();
    settle(cx, w, 4);
}

/// An agent message of three rows or more: its labelled row, that row's
/// id, and the next labelled row (the first such: the most history under it).
fn long_agent_message(cx: &mut TestAppContext, view: &Entity<AppView>) -> (usize, u64, usize) {
    use crate::turns::Who;
    view.read_with(cx, |v, _| {
        let labels: Vec<usize> = (0..v.disp.len()).filter(|&i| v.disp[i].who != Who::None).collect();
        labels
            .windows(2)
            .find(|p| v.disp[p[0]].who == Who::Agent && p[1] - p[0] > 2)
            .map(|p| (p[0], v.disp[p[0]].id, p[1]))
            .expect("a long agent message")
    })
}

/// `sel`'s bounds as last drawn: left, top, width, height.
fn drawn(cx: &mut TestAppContext, w: AnyWindowHandle, sel: String) -> Option<[f32; 4]> {
    let b = VisualTestContext::from_window(w, cx).debug_bounds(sel.leak())?;
    Some([f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height)])
}

fn near(a: [f32; 4], b: [f32; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-3)
}

/// The list's viewport: left, top.
fn viewport(cx: &mut TestAppContext, view: &Entity<AppView>) -> (f32, f32) {
    view.read_with(cx, |v, _| {
        let b = v.list.viewport_bounds();
        (f32::from(b.origin.x), f32::from(b.origin.y))
    })
}

const LEVELS: [crate::motion::Level; 3] = [crate::motion::Level::Full, crate::motion::Level::Reduced, crate::motion::Level::Off];

/// P15: the top message's label sticks in the gutter at the viewport's top
/// + `pad_y` once it would scroll above it, exactly where and as its slot
/// drew it there; while the label is at the pin or below, only the
/// original is drawn. The same at Full, Reduced and Off (placement, not
/// motion), and it asks for no frame.
#[gpui_kit::test]
fn sticky_label_is_topmost_message(cx: &mut TestAppContext) {
    let (w, view, _events) = sticky_open(cx, (2200., 640.));
    assert!(view.read_with(cx, |v, _| v.layout.hang), "the labels hang");
    let (l, id, next) = long_agent_message(cx, &view);
    let pad_y = view.read_with(cx, |_, cx| crate::theme::metrics(cx).pad_y);
    let (slot, pinned) = (format!("hang-label-{id}"), format!("sticky {id}"));
    // Its labelled row on top: the label in its slot, below the pin.
    scroll_list(cx, w, &view, l, 0.);
    let (_, top) = viewport(cx, &view);
    let below = drawn(cx, w, slot.clone()).expect("the label in its slot");
    assert!(drawn(cx, w, pinned.clone()).is_none(), "nothing pinned");
    // Scrolled until it is at the pin: still only the original.
    let off = below[1] - top - pad_y;
    assert!(off > 0., "{below:?}");
    scroll_list(cx, w, &view, l, off);
    let at_pin = drawn(cx, w, slot.clone()).expect("the label in its slot");
    assert!((at_pin[1] - (top + pad_y)).abs() < 1e-3, "{at_pin:?}");
    assert!(drawn(cx, w, pinned.clone()).is_none(), "at the pin: nothing pinned");
    for level in LEVELS {
        set_motion(cx, &view, level);
        // A pixel past it: pinned where the slot was, as it was.
        scroll_list(cx, w, &view, l, off + 1.);
        let stuck = drawn(cx, w, pinned.clone()).expect("pinned");
        assert!(near(stuck, at_pin), "{level:?}: {stuck:?} is the slot at the pin {at_pin:?}");
        // Every other row of the message on top: the same.
        for ix in l + 1..next {
            scroll_list(cx, w, &view, ix, 0.);
            assert!(drawn(cx, w, pinned.clone()).is_some_and(|b| near(b, at_pin)), "{level:?}: row {ix} on top");
        }
    }
    assert_eq!(settle(cx, w, 8), 0, "the pinned label asks for no frame");
}

/// P15: the next message's label pushes the pinned one up by exactly the
/// overlap, following the scroll 1:1 (no spring), at every level, until
/// it is out of the top.
#[gpui_kit::test]
fn sticky_label_pushed_by_next(cx: &mut TestAppContext) {
    let (w, view, _events) = sticky_open(cx, (2200., 640.));
    let (l, id, next) = long_agent_message(cx, &view);
    let next_id = view.read_with(cx, |v, _| v.disp[next].id);
    let pad_y = view.read_with(cx, |_, cx| crate::theme::metrics(cx).pad_y);
    let (pinned, their) = (format!("sticky {id}"), format!("hang-label-{next_id}"));
    // The next label's place in its row, and the pinned label's height.
    scroll_list(cx, w, &view, next, 0.);
    let (_, top) = viewport(cx, &view);
    let nat = drawn(cx, w, their.clone()).expect("the next label")[1] - top;
    scroll_list(cx, w, &view, l + 1, 0.);
    let h = drawn(cx, w, pinned.clone()).expect("pinned")[3];
    for level in LEVELS {
        set_motion(cx, &view, level);
        // The next label 8 px into the pinned one: that one is 8 px up.
        scroll_list(cx, w, &view, next, 0.);
        scroll_list_by(cx, w, &view, -(pad_y + h - 8. - nat));
        let on_top = view.read_with(cx, |v, _| v.list.logical_scroll_top().item_ix);
        // (The next label's own row, under its gap, may be on top already.)
        assert!(on_top > l && on_top <= next, "{level:?}: row {on_top} on top");
        let (stuck, theirs) = (drawn(cx, w, pinned.clone()).expect("pinned"), drawn(cx, w, their.clone()).expect("the next label"));
        assert!((theirs[1] - (top + pad_y + h - 8.)).abs() < 1e-3, "{level:?}: {theirs:?}");
        assert!((stuck[1] - (top + pad_y - 8.)).abs() < 1e-3, "{level:?}: pushed by the overlap: {stuck:?}");
        // On up, 1:1 with the scroll, touching it, until it is out.
        let mut last = stuck[1];
        for k in 0.. {
            scroll_list_by(cx, w, &view, 5.);
            let theirs = drawn(cx, w, their.clone()).expect("the next label");
            let Some(stuck) = drawn(cx, w, pinned.clone()) else {
                assert!((theirs[1] - top).abs() < 1e-3, "{level:?}: gone once the next label reaches the top ({theirs:?})");
                break;
            };
            assert!((stuck[1] - (last - 5.)).abs() < 1e-3, "{level:?} step {k}: {stuck:?} after {last}");
            assert!((stuck[1] + h - theirs[1]).abs() < 1e-3, "{level:?} step {k}: touching {theirs:?}");
            last = stuck[1];
            assert!(k < 20, "{level:?}: pushed out within {h} px");
        }
    }
}

/// P15 F8: with the labels inline (a narrow pane), the label line sticks
/// at the column's top on a `canvas` band one line tall, a 1 px `hair`
/// rule under it, only while the original line is scrolled off.
#[gpui_kit::test]
fn sticky_label_on_a_band_when_inline(cx: &mut TestAppContext) {
    let (w, view, _events) = sticky_open(cx, (1440., 900.));
    assert!(!view.read_with(cx, |v, _| v.layout.hang), "the labels are inline");
    let (l, id, _) = long_agent_message(cx, &view);
    let (line, column, left) = view.read_with(cx, |v, cx| (crate::theme::metrics(cx).body_line, v.layout.column, v.layout.left));
    let pinned = format!("sticky {id}");
    scroll_list(cx, w, &view, l, 0.);
    let (x, top) = viewport(cx, &view);
    let off = drawn(cx, w, format!("label {id}")).expect("the label line")[1] - top;
    for level in LEVELS {
        set_motion(cx, &view, level);
        scroll_list(cx, w, &view, l, off);
        assert!(drawn(cx, w, pinned.clone()).is_none(), "{level:?}: the line at the top: nothing pinned");
        scroll_list(cx, w, &view, l, off + 1.);
        let band = drawn(cx, w, pinned.clone()).expect("the band");
        assert!(near(band, [x + left, top, column, line]), "{level:?}: {band:?}");
        let rule = drawn(cx, w, format!("sticky-rule {id}")).expect("the rule");
        assert!(near(rule, [x + left, top + line, column, 1.]), "{level:?}: {rule:?}");
    }
}

/// P15: the pinned label never takes the wheel: it scrolls the list under
/// it. F8's band hides live rows, so it takes the pointer (Shady #19 M1): a
/// click on it reaches nothing under it (a folded run's header there stays
/// folded); 1 px under it, the click unfolds the run. (Hanging, the label
/// sits on the empty gutter, with no hitbox at all.)
#[gpui_kit::test]
fn sticky_label_takes_no_click(cx: &mut TestAppContext) {
    use gpui_kit::{
        Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ScrollDelta, ScrollEvent, ScrollSource, TouchPhase, point,
    };
    let (w, view, _events) = sticky_open(cx, (1440., 900.));
    set_motion(cx, &view, crate::motion::Level::Reduced);
    let (run, ix, msg) = view.read_with(cx, |v, _| {
        let ix = v.disp.iter().position(|d| matches!(d.kind, crate::turns::Kind::Summary { open: false, .. })).expect("a folded run");
        let msg = (0..=ix).rev().find(|&i| v.disp[i].who != crate::turns::Who::None).expect("its message");
        (v.disp[ix].id, ix, v.disp[msg].id)
    });
    let (summary, line) = view.read_with(cx, |_, cx| (crate::theme::metrics(cx).summary, crate::theme::metrics(cx).body_line));
    scroll_list(cx, w, &view, ix, 0.);
    let (_, top) = viewport(cx, &view);
    let row = view.read_with(cx, |v, _| v.list.bounds_for_item(ix)).expect("drawn");
    // Its header (the row's last `summary` px) from 4 px above the top.
    let under = |cx: &mut TestAppContext| {
        scroll_list(cx, w, &view, ix, f32::from(row.size.height) - summary + 4.);
        let band = drawn(cx, w, format!("sticky {msg}")).expect("the band over the header");
        let at = point(row.center().x, px(top + line / 2.));
        assert!(band[1] <= top && band[1] + band[3] > f32::from(at.y), "{band:?}");
        at
    };
    let offset = |cx: &mut TestAppContext| view.read_with(cx, |v, _| -f32::from(v.list.scroll_px_offset_for_scrollbar().y));
    // The wheel over the band scrolls the list (Reduced: a notch, at once).
    let at = under(cx);
    let before = offset(cx);
    event(cx, w, PlatformInput::MouseMove(MouseMoveEvent { position: at, pressed_button: None, modifiers: Modifiers::default() }));
    event(
        cx,
        w,
        PlatformInput::Scroll(ScrollEvent {
            position: at,
            delta: ScrollDelta::Lines(point(0., -3.)),
            modifiers: Modifiers::default(),
            touch_phase: TouchPhase::Moved,
            source: ScrollSource::Pointer,
        }),
    );
    settle(cx, w, 2);
    assert_eq!(offset(cx) - before, 60., "the wheel went through");
    let click = |cx: &mut TestAppContext, at: gpui_kit::Point<gpui_kit::Pixels>| {
        let mut vcx = VisualTestContext::from_window(w, cx);
        vcx.simulate_event(MouseMoveEvent { position: at, pressed_button: None, modifiers: Modifiers::default() });
        vcx.simulate_event(MouseDownEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        });
        vcx.simulate_event(MouseUpEvent { button: MouseButton::Left, position: at, modifiers: Modifiers::default(), click_count: 1 });
        vcx.run_until_parked();
        settle(cx, w, 4);
        view.read_with(cx, |v, _| v.disp.iter().any(|d| d.id == run && matches!(d.kind, crate::turns::Kind::Summary { open: true, .. })))
    };
    // A click on the band: the run under it stays folded.
    let at = under(cx);
    assert!(!click(cx, at), "the band took the click");
    // 1 px under the band and its rule, on the header: it unfolds.
    let at = point(at.x, px(top + line + 2.));
    assert!(drawn(cx, w, format!("sticky-rule {msg}")).is_some_and(|r| r[1] + r[3] < f32::from(at.y)), "under the rule");
    assert!(click(cx, at), "the click reached the run's header");
}

/// F8 (Shady #19 M2): at Full, the band hides the beat under it. Running
/// rows scrolled under it keep their glyphs and orbits on the beat layer
/// (painted after the app's whole frame, so over the band), but no site's
/// clip meets the band: their beats show only below it.
#[gpui_kit::test]
fn sticky_band_hides_the_beat(cx: &mut TestAppContext) {
    let (w, view, _events) = sticky_open(cx, (1440., 900.));
    assert!(!view.read_with(cx, |v, _| v.layout.hang), "the labels are inline");
    set_motion(cx, &view, crate::motion::Level::Full);
    running_tools(cx, &view, 60);
    for _ in 0..12 {
        beat(cx, w, &view);
    }
    // A running row (the last 60 rows are) with screens of them under it,
    // 6 px out of the top: its message's label is above it.
    let (ix, id) = view.read_with(cx, |v, _| {
        let ix = v.disp.len() - 40;
        let l = (0..ix).rev().find(|&i| v.disp[i].who != crate::turns::Who::None).expect("the running message");
        (ix, v.disp[l].id)
    });
    let line = view.read_with(cx, |_, cx| crate::theme::metrics(cx).body_line);
    scroll_list(cx, w, &view, ix, 6.);
    beat(cx, w, &view);
    let (_, top) = viewport(cx, &view);
    let band = drawn(cx, w, format!("sticky {id}")).expect("the band");
    let cover = Bounds::new(gpui_kit::point(px(band[0]), px(top)), size(px(band[2]), px(line + 1.)));
    assert_eq!(band[1], top, "{band:?}");
    let sites = view.read_with(cx, |v, _| v.fx.sites());
    let under = sites
        .iter()
        .filter(|s| matches!(s.beat, crate::motion::Beat::Spin(_) | crate::motion::Beat::Lattice(_)) && s.origin.y < cover.bottom())
        .count();
    assert!(under >= 1, "a running glyph under the band: {sites:?}");
    for s in &sites {
        assert!(!s.mask.bounds.intersects(&cover), "a beat over the band: {s:?}, band {cover:?}");
    }
    // Under the band, the next rows' glyphs still beat.
    assert!(sites.iter().any(|s| s.mask.bounds.size.height > px(0.) && s.origin.y > cover.bottom()), "the rest beat on");
}

/// Q2 L1 (Shady #19): with the labels inline, Ctrl+J lands a card scrolled
/// out of the top below the sticky band, not under it: its top edge (F7's
/// dashed mark, the trace) shows. (Calls under the card, so it can be
/// scrolled out of the top.)
#[gpui_kit::test]
fn ctrl_j_lands_the_card_below_the_band(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    assert!(!view.read_with(cx, |v, _| v.layout.hang), "the labels are inline");
    running_tools(cx, &view, 30);
    settle(cx, w, 8);
    let (ix, msg, line) = view.read_with(cx, |v, cx| {
        let ix = v.card_ix().expect("a card");
        assert!(ix + 30 < v.disp.len(), "the calls under the card");
        let msg = (0..ix).rev().find(|&i| v.disp[i].who != crate::turns::Who::None).expect("its message's label above it");
        (ix, v.disp[msg].id, crate::theme::metrics(cx).body_line)
    });
    view.update(cx, |v, cx| {
        v.list.scroll_to(gpui_kit::ListOffset { item_ix: ix, offset_in_item: px(60.) });
        cx.notify();
    });
    settle(cx, w, 8);
    settle(cx, w, 8);
    focus_composer(cx, w, &view);
    tap(cx, w, "ctrl-j");
    for _ in 0..120 {
        fluid_frame(cx, w, &view);
    }
    settle(cx, w, 8);
    let (_, top) = viewport(cx, &view);
    let card = view.read_with(cx, |v, _| v.list.bounds_for_item(ix)).expect("the card drawn");
    assert!((f32::from(card.top()) - (top + line + 1.)).abs() < 0.5, "the card's top {:?}, the view's {top}", card.top());
    let band = drawn(cx, w, format!("sticky {msg}")).expect("the band");
    let rule = drawn(cx, w, format!("sticky-rule {msg}")).expect("the rule");
    assert!(band[1] == top && rule[1] + rule[3] <= f32::from(card.top()) + 0.5, "{band:?} {rule:?} {card:?}");
}

/// polish.md P15: F1's four transcript tests and the unfold-hold tests
/// run unchanged with the sticky layer: each, with its attributes (an added
/// `#[ignore]` is a change), and every helper they call, all the way down,
/// is byte for byte main `0a78bb7`'s (FNV-1a 64 of its text, its first
/// attribute or `fn` to its closing brace).
#[test]
fn scroll_tests_unchanged() {
    let src = include_str!("ui_tests.rs").replace("\r\n", "\n");
    let text = |name: &str| {
        let at = src.find(&format!("\nfn {name}(")).or_else(|| src.find(&format!("\nfn {name}<"))).unwrap_or_else(|| panic!("{name}")) + 1;
        let mut start = at;
        while let Some(prev) = src[..start - 1].rfind('\n').map(|i| i + 1) {
            if !src[prev..start].starts_with("#[") {
                break;
            }
            start = prev;
        }
        let end = src[at..].find("\n}\n").unwrap_or_else(|| panic!("{name}")) + at + 2;
        src[start..end].to_string()
    };
    let fnv = |s: &str| s.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    for (name, main) in [
        ("follow_offset_is_end_every_frame", 0xa949_8145_0e4f_434f_u64),
        ("anchor_row_never_moves", 0xd93f_4109_d027_0d82),
        ("transcript_heights_stable_across_frames", 0x965d_5b2a_f1a0_6043),
        ("history_never_animates", 0x72d1_37a0_9793_8774),
        ("an_unfold_under_full_keeps_its_header_put", 0x4a16_3437_267f_75aa),
        ("a_fold_drops_its_rows_from_the_unfold_hold", 0x1661_ce04_ee7a_42a2),
        // The helpers they call.
        ("a_folded_run", 0x19bc_8ea1_9ee7_246c),
        ("attached", 0x121a_81e7_74c9_398c),
        ("fluid_frame", 0xbbfc_1d0b_594d_4177),
        ("frame", 0xb938_3690_352a_5ffb),
        ("frames", 0x78aa_3c16_3afd_0efe),
        ("motion_pending", 0x8743_a6e6_23ae_a4c0),
        ("open", 0xcf0d_a8e3_2704_f4a4),
        ("open_at", 0x280c_3155_84e0_4e3c),
        ("open_idle_cpu", 0xc019_bf9d_983b_e771),
        ("open_with", 0x6980_8528_8613_00f9),
        ("set_motion", 0x7be3_0ca6_f053_6887),
        ("settle", 0xb6b3_4f31_3ec9_964a),
        ("start_stream", 0xd6a2_22bc_4ece_7d58),
        ("stream_step", 0xbfc1_6760_b75a_fb66),
        ("wait", 0x1217_4e34_2ad1_06e7),
    ] {
        assert_eq!(fnv(&text(name)), main, "{name} changed");
    }
}

// ── engine events (BRIEF-events) ──────────────────────────────────────────

/// A synthetic `External` frame (agent-engine `session/wire.rs`).
fn external_frame(id: &str, text: &str, sev: &str, ask: bool) -> serde_json::Value {
    serde_json::json!({"ev": "external", "event": {
        "id": id, "timestamp": "2031-02-03T04:05:06Z",
        "source": {"source_type": "agent-comms", "name": "agent-comms", "callback": null},
        "channel": {"id": "comms", "name": "comms"},
        "sender": {"id": "sid-0001", "name": "kestrel", "sender_type": "agent"},
        "content": {"text": text, "content_type": "message", "severity": sev, "data": null},
        "expects_response": ask, "reply_to": null}})
}

/// `raw` as the attached session's live frame, through the model.
fn event_in(cx: &mut TestAppContext, view: &Entity<AppView>, raw: serde_json::Value) {
    view.update(cx, |v, cx| {
        let (seq, sid) = v.model.active.as_ref().map(|a| (a.attach_seq, a.session_id.clone())).expect("attached");
        let event = synaps_client::protocol::decode_event(&raw).expect("decodes");
        let ev = synaps_client::Event::session(seq, sid, 1, event, std::sync::Arc::new(raw)).with_ts("2031-02-03T04:05:07Z");
        v.apply_batch(vec![ev], cx);
        cx.notify();
    });
}

fn kinds_of(cx: &mut TestAppContext, view: &Entity<AppView>) -> Vec<synaps_client::transcript::ItemKind> {
    view.read_with(cx, |v, _| v.model.shown().items().iter().map(|i| i.kind).collect())
}

/// Scrolled back, events landing (each severity, and one's steer into the
/// turn) never move a row on screen and never resume following; each is
/// one event row, never a user row.
#[gpui_kit::test]
fn an_event_never_moves_a_scrolled_up_view(cx: &mut TestAppContext) {
    use synaps_client::transcript::ItemKind;
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    view.update(cx, |v, _| v.client = None);
    start_stream(cx, &view);
    settle(cx, w, 8);
    for k in 0..30 {
        stream_step(cx, &view, k);
        fluid_frame(cx, w, &view);
    }
    view.update(cx, |v, _| v.list.scroll_by(gpui_kit::px(-260.)));
    fluid_frame(cx, w, &view);
    let users = kinds_of(cx, &view).iter().filter(|k| **k == ItemKind::User).count();
    let (anchor, top) = view.read_with(cx, |v, _| {
        let ix = v.list.logical_scroll_top().item_ix + 1;
        (ix, v.list.bounds_for_item(ix).map(|b| b.top()))
    });
    assert!(top.is_some(), "the anchor row is on screen");
    for (k, sev) in ["Low", "Medium", "High", "Critical"].into_iter().enumerate() {
        let id = format!("ev-scroll-{k}");
        event_in(cx, &view, external_frame(&id, "[comms] #3 ask from kestrel: still there?", sev, k % 2 == 0));
        let form = format!(
            "<event id=\"{id}\" type=\"message\" severity=\"{}\" source=\"agent-comms\" channel=\"comms\">[comms] #3 ask from kestrel: still there?</event>",
            sev.to_lowercase()
        );
        event_in(
            cx,
            &view,
            serde_json::json!({"ev": "stream", "event": {"kind": "agent", "agent": "steering_delivered", "message": form}}),
        );
        for _ in 0..12 {
            stream_step(cx, &view, 30 + k);
            fluid_frame(cx, w, &view);
            let (following, now) = view.read_with(cx, |v, _| (v.list.is_following_tail(), v.list.bounds_for_item(anchor).map(|b| b.top())));
            assert!(!following, "{sev}: still scrolled back");
            assert_eq!(now, top, "{sev}: the anchor row moved");
        }
    }
    let kinds = kinds_of(cx, &view);
    assert_eq!(kinds.iter().filter(|k| **k == ItemKind::Event).count(), 4, "one row per event");
    assert_eq!(kinds.iter().filter(|k| **k == ItemKind::User).count(), users, "no user row");
}

/// An event row reads its source, sender, ASK and time; a long one folds
/// to its first lines and opens on a click; and after it arrives the
/// window goes idle (no frame, no timer but the rail's).
#[gpui_kit::test]
fn an_event_row_folds_and_goes_idle(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    view.update(cx, |v, _| v.client = None);
    open_idle_cpu(cx, &view, crate::tabs::Place::Current);
    let long = (1..=24).map(|i| format!("step {i} of the synthetic rollout")).collect::<Vec<_>>().join("\n");
    event_in(cx, &view, external_frame("ev-fold", &format!("[comms] #12 ask from kestrel: {long}"), "High", true));
    view.update(cx, |v, cx| {
        v.sync_list();
        v.list.scroll_to_end();
        cx.notify();
    });
    let mut n = 0;
    while motion_pending(cx, &view).0 && n < 400 {
        fluid_frame(cx, w, &view);
        n += 1;
    }
    settle(cx, w, 8);
    let id = view.read_with(cx, |v, _| v.model.shown().items().last().map(|i| i.id)).unwrap();
    for sel in ["event", "event-ask", "event-fold", "time"] {
        assert!(drawn(cx, w, format!("{sel} {id}")).is_some(), "{sel} drawn");
    }
    let folded = drawn(cx, w, format!("event-text {id}")).unwrap();
    let fold = drawn(cx, w, format!("event-fold {id}")).unwrap();
    click_at(cx, w, gpui_kit::point(gpui_kit::px(fold[0] + 4.), gpui_kit::px(fold[1] + fold[3] / 2.)));
    view.update(cx, |v, _| v.list.scroll_to_end());
    settle(cx, w, 8);
    let open_h = drawn(cx, w, format!("event-text {id}")).unwrap();
    assert!(open_h[3] > folded[3] * 2., "opened: {open_h:?} vs {folded:?}");
    assert!(view.read_with(cx, |v, _| v.toggled.contains(&id)));
    let mut n = 0;
    while motion_pending(cx, &view).0 && n < 400 {
        fluid_frame(cx, w, &view);
        n += 1;
    }
    assert!(settle(cx, w, 8) <= 2);
    wait(cx, w, &view, crate::motion::GLOW.as_millis() as u64);
    only_the_rail_spins(cx, w, &view, "after an event");
}

// ── Q5 Panels (polish.md P13's dock half, P14; fluid.md §3.7–3.8) ──────

/// A side panel's shown width (`rail`, else the right one), its target
/// (`sides`), and whether it flies.
fn panel(cx: &mut TestAppContext, view: &Entity<AppView>, rail: bool) -> (f32, f32, bool) {
    view.read_with(cx, |v, _| {
        let f = v.panel_fx.sides[usize::from(!rail)];
        let (l, r) = crate::app::sides(v.width, v.panels, crate::chrome::MARK);
        (f.shown, if rail { l.width() } else { r.width() }, f.moving)
    })
}

/// The pointer moved to `x` (y 400) with the left button held.
fn drag_to(cx: &mut TestAppContext, w: AnyWindowHandle, x: f32) {
    let ev = gpui_kit::MouseMoveEvent {
        position: gpui_kit::point(px(x), px(400.)),
        pressed_button: Some(gpui_kit::MouseButton::Left),
        modifiers: Default::default(),
    };
    event(cx, w, PlatformInput::MouseMove(ev));
}

/// A panel's flight from now to rest, on motion's clock (the test's
/// frames can stall on a timer's instant, so time, not frames, is what a
/// token sets): its length in ms, and each drawn frame's width.
fn flight(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, rail: bool) -> (u128, Vec<f32>) {
    let t0 = view.read_with(cx, |v, _| v.fx.frame_now());
    let mut widths = vec![panel(cx, view, rail).0];
    let mut end = t0;
    for _ in 0..400 {
        if !panel(cx, view, rail).2 {
            break;
        }
        fluid_frame(cx, w, view);
        let (s, _, moving) = panel(cx, view, rail);
        widths.push(s);
        if !moving {
            end = view.read_with(cx, |v, _| v.fx.frame_now());
        }
    }
    (end.duration_since(t0).as_millis(), widths)
}

/// The motion clock brought up to the last timer that woke the app: a
/// test's setup can run the executor's clock ahead of motion's (a timer
/// then fires early and every frame samples its instant until the clock
/// gets there, then jumps).
fn catch_up(cx: &mut TestAppContext, view: &Entity<AppView>) {
    view.read_with(cx, |v, _| v.fx.set_clock(v.fx.tick()));
}

/// Fluid frames until nothing moves (at most `max`): how many. A panel in
/// its slow tail asks for a timer, not a frame (`Motion::spring_px`): it
/// moves until it lands.
fn fly(
    cx: &mut TestAppContext,
    w: AnyWindowHandle,
    view: &Entity<AppView>,
    max: usize,
    mut each: impl FnMut(&mut TestAppContext),
) -> usize {
    let mut n = 0;
    while (motion_pending(cx, view).0 || view.read_with(cx, |v, _| v.panel_fx.moving())) && n < max {
        fluid_frame(cx, w, view);
        each(cx);
        n += 1;
    }
    n
}

/// fluid.md §3.8: Ctrl+B folds the rail on `sheet` (426 ms to rest: ~61
/// frames at 144 Hz), from its width to the strip, never past either
/// (critically damped); in flight the rail is drawn at its open width in a
/// box as wide as the spring, clipped (not squeezed). Then it rests: no
/// frame. Unfolding is the same, back. Reduced and Off: in one frame.
#[gpui_kit::test]
fn panels_fold_on_sheet_clipped_not_squeezed(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    settle(cx, w, 8);
    for rail in [true, false] {
        let (open_w, _, _) = panel(cx, &view, rail);
        for fold in [true, false] {
            view.update(cx, |v, cx| v.toggle_panel(rail, cx));
            cx.run_until_parked();
            let (from, to) = if fold { (open_w, crate::app::STRIP) } else { (crate::app::STRIP, open_w) };
            let (first, target, moving) = panel(cx, &view, rail);
            assert_eq!(target, to, "{rail} {fold}");
            assert!(moving && (first - from).abs() < 1., "{rail} {fold}: it starts where it was ({first})");
            let mut last = first;
            let mut boxed = 0;
            let t0 = view.read_with(cx, |v, _| v.fx.frame_now());
            let mut end = t0;
            fly(cx, w, &view, 400, |cx| {
                let (s, _, moving) = panel(cx, &view, rail);
                assert!(s >= from.min(to) - 1e-3 && s <= from.max(to) + 1e-3, "{rail} {fold}: {s} outside {from}..{to}");
                assert!(if fold { s <= last + 1e-3 } else { s >= last - 1e-3 }, "{rail} {fold}: {s} after {last}");
                last = s;
                if !moving && end == t0 {
                    end = view.read_with(cx, |v, _| v.fx.frame_now());
                }
                if moving {
                    let sel = format!("{}-flight", if rail { "rail" } else { "right" });
                    let b = drawn(cx, w, sel).expect("the flight's box");
                    assert!((b[2] - s).abs() < 0.5, "{rail} {fold}: the box is the spring's width ({b:?} {s})");
                    boxed += 1;
                }
            });
            assert!(boxed > 10, "{rail} {fold}: drawn clipped in flight ({boxed})");
            // `sheet`: 426 ms to rest over 100 px, a little more over 280.
            let ms = end.duration_since(t0).as_millis();
            assert!((420..=600).contains(&ms), "{rail} {fold}: {ms} ms on `sheet`");
            assert_eq!(panel(cx, &view, rail), (to, to, false), "{rail} {fold}: at rest on its target");
            assert!(settle(cx, w, 8) <= 2);
            assert!(!motion_pending(cx, &view).0, "{rail} {fold}: no frame at rest");
        }
    }
    for level in [crate::motion::Level::Reduced, crate::motion::Level::Off] {
        set_motion(cx, &view, level);
        settle(cx, w, 8);
        for _ in 0..2 {
            view.update(cx, |v, cx| v.toggle_panel(true, cx));
            cx.run_until_parked();
            let (s, t, moving) = panel(cx, &view, true);
            assert!(s == t && !moving, "{level:?}: in one frame");
            assert!(!motion_pending(cx, &view).0, "{level:?}: no frame");
        }
    }
}

/// fluid.md §3.8: while a panel flies the column keeps its wrap: it is
/// laid out for the flight's narrowest pane, so it rewraps at most once
/// per flight (at the start when the pane narrows, at the end when it
/// widens), and is centred in the pane as drawn every frame. Not following,
/// the top row's top never moves; following, the end stays the bottom.
#[gpui_kit::test]
fn column_rewraps_at_most_once_per_panel_motion(cx: &mut TestAppContext) {
    let (w, view, _events) = open_at(cx, (1100., 800.), crate::theme::Panels::default().text_size);
    set_motion(cx, &view, crate::motion::Level::Full);
    settle(cx, w, 8);
    let col = |cx: &mut TestAppContext| {
        view.read_with(cx, |v, _| {
            let l = v.layout;
            let pane = v.width - crate::chrome::MARK - v.panel_fx.sides[0].shown - v.panel_fx.sides[1].shown;
            (l.column, l.hang, l.left, pane)
        })
    };
    for (follow, fold) in [(false, true), (false, false), (true, true), (true, false)] {
        if follow {
            view.update(cx, |v, _| v.list.scroll_to_end());
        } else {
            // Far enough back that what is under the top row fills the view
            // at either wrap (near the end, a shorter column clamps).
            view.update(cx, |v, _| v.list.scroll_to(gpui_kit::ListOffset { item_ix: 2, offset_in_item: px(13.) }));
        }
        frame(cx, &view);
        settle(cx, w, 8);
        let before = col(cx);
        let anchor = view.read_with(cx, |v, _| v.list.logical_scroll_top().item_ix);
        let top = view.read_with(cx, |v, _| v.list.bounds_for_item(anchor).map(|b| b.top()));
        view.update(cx, |v, cx| v.toggle_panel(true, cx));
        cx.run_until_parked();
        let mut seen = vec![col(cx)];
        let mut moving = vec![panel(cx, &view, true).2];
        let check = |cx: &mut TestAppContext, seen: &mut Vec<(f32, bool, f32, f32)>| {
            let c = col(cx);
            assert!((c.2 - ((c.3 - c.0) / 2.).floor()).abs() <= 1., "{fold}: centred in the pane as drawn {c:?}");
            // GPUI's item width (the list's width less its padding) is the
            // column, bit for bit, in every frame (Shady H1: a fraction of a
            // pixel more or less and the list forgets every row's height).
            let item = view.read_with(cx, |v, _| {
                let ix = if follow { v.disp.len() - 1 } else { anchor };
                v.list.bounds_for_item(ix).map(|b| f32::from(b.size.width))
            });
            assert_eq!(item, Some(c.0), "{fold}: the list's item width is the column, frame {}", seen.len());
            // (`seen` starts with this frame's own: the first compares
            // with the frame before the flight.)
            let rewrapped = (seen.len() == 1 && c.0 != before.0) || seen.last().is_some_and(|p| p.0 != c.0);
            seen.push(c);
            if follow {
                let (following, end) = view.read_with(cx, |v, _| (v.list.is_following_tail(), v.list.is_scrolled_to_end()));
                // Every height known (none dropped), at the end; the frame
                // that rewraps measures again.
                assert!(following && (end == Some(true) || rewrapped), "{fold}: following, at the end ({end:?})");
            } else {
                let now = view.read_with(cx, |v, _| v.list.bounds_for_item(anchor).map(|b| b.top()));
                assert_eq!(now, top, "{fold}: the top row moved, frame {}", seen.len());
            }
        };
        check(cx, &mut seen);
        let frames = fly(cx, w, &view, 400, |cx| {
            check(cx, &mut seen);
            moving.push(panel(cx, &view, true).2);
        });
        assert!(frames > 30, "{fold}: it flew ({frames})");
        let wraps: Vec<(f32, bool)> = std::iter::once((before.0, before.1)).chain(seen.iter().map(|c| (c.0, c.1))).collect();
        let changes: Vec<usize> = (1..wraps.len()).filter(|&i| wraps[i] != wraps[i - 1]).collect();
        assert!(changes.len() <= 1, "{fold}: rewrapped {} times: {wraps:?}", changes.len());
        assert!(!changes.is_empty(), "{fold}: 1100 px crosses a wrap width either way");
        if fold {
            // The pane widens: the new wrap in the flight's last frame
            // (`wraps` has the frame before the flight first).
            let landed = moving.iter().position(|m| !m).expect("it landed");
            assert!((landed + 1..=landed + 2).contains(&changes[0]), "{fold}: widening rewraps at the end ({} {landed})", changes[0]);
        } else {
            assert_eq!(changes[0], 1, "{fold}: narrowing rewraps at the start");
        }
    }
}

/// Q5: what is placed from the panes follows a panel in flight in its
/// frame: Q7's pinned label stays pinned, on its band on the column as drawn
/// while inline, and Q6's palette stays centred on the centre pane as drawn, under
/// the tab strip, every frame of a rail fold and unfold.
#[gpui_kit::test]
fn sticky_and_palette_follow_a_panel_in_flight(cx: &mut TestAppContext) {
    let (w, view, _events) = sticky_open(cx, (1440., 900.));
    set_motion(cx, &view, crate::motion::Level::Full);
    let (l, id, _) = long_agent_message(cx, &view);
    let pinned = format!("sticky {id}");
    scroll_list(cx, w, &view, l, 0.);
    let off = drawn(cx, w, format!("label {id}")).expect("the label line")[1] - viewport(cx, &view).1;
    scroll_list(cx, w, &view, l, off + 1.);
    let line = view.read_with(cx, |_, cx| crate::theme::metrics(cx).body_line);
    let mut bands = 0;
    for _ in 0..2 {
        view.update(cx, |v, cx| v.toggle_panel(true, cx));
        cx.run_until_parked();
        let frames = fly(cx, w, &view, 400, |cx| {
            let (x, top) = viewport(cx, &view);
            let (hang, left, column) = view.read_with(cx, |v, _| (v.layout.hang, v.layout.left, v.layout.column));
            let band = drawn(cx, w, pinned.clone()).expect("pinned in every frame");
            // Hanging (the pane wide enough, once the fold lands), the label
            // sits in the gutter: Q7's own tests place it.
            if !hang {
                bands += 1;
                assert!(near(band, [x + left, top, column, line]), "on the column as drawn: {band:?} {x} {left} {column}");
            }
        });
        assert!(frames > 30, "{frames}");
    }
    assert!(bands > 30, "inline in flight: {bands}");
    focus_composer(cx, w, &view);
    tap(cx, w, palette_key(false));
    frame(cx, &view);
    assert!(palette_mode(cx, &view).is_some(), "open");
    for _ in 0..2 {
        view.update(cx, |v, cx| v.toggle_panel(true, cx));
        cx.run_until_parked();
        let frames = fly(cx, w, &view, 400, |cx| {
            view.read_with(cx, |v, _| {
                let left = crate::chrome::MARK + v.panel_fx.sides[0].shown;
                let pane = v.width - left - v.panel_fx.sides[1].shown;
                let (_, _, x, pw) = v.palette_box.get();
                let strip = f32::from(v.strip_at.get().y) + crate::tabstrip::TAB_H;
                assert!((x + pw / 2. - (left + pane / 2.)).abs() <= 0.5, "centred on the pane as drawn: {x} {pw} {left} {pane}");
                assert!(v.palette_box.get().0 >= strip + 8., "under the strip");
            })
        });
        assert!(frames > 30, "{frames}");
    }
    assert!(palette_mode(cx, &view).is_some(), "still open");
}

/// fluid.md §3.8: a drag follows the pointer 1:1 (no spring), in every frame.
#[gpui_kit::test]
fn drag_is_one_to_one(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    settle(cx, w, 8);
    let (w0, _, _) = panel(cx, &view, true);
    press_at(cx, w, gpui_kit::point(px(w0 - 1.), px(400.)));
    // GPUI starts the drag on the first move.
    drag_to(cx, w, w0 + 2.);
    for (k, x) in [w0 + 10., w0 + 40., w0 + 90., w0 + 60., w0 + 130.].into_iter().enumerate() {
        drag_to(cx, w, x - 1.);
        let (s, t, moving) = panel(cx, &view, true);
        assert_eq!(t, (x).round(), "move {k}: the panel is where the pointer is");
        assert!(s == t && !moving, "move {k}: drawn there in the same frame ({s} {t})");
    }
    release_at(cx, w, gpui_kit::point(px(w0 + 129.), px(400.)));
    assert_eq!(panel(cx, &view, true), (w0 + 130., w0 + 130., false), "let go far from a detent: it stays");
    assert!(!motion_pending(cx, &view).0);
}

/// fluid.md §3.8: let go within 24 px of a detent, the panel springs there
/// on `snap` (the auto width: auto again); further, it stays. A
/// double-click on the edge springs to the auto width on `sheet`.
#[gpui_kit::test]
fn detent_settles(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    settle(cx, w, 8);
    let (w0, _, _) = panel(cx, &view, true);
    assert_eq!(view.read_with(cx, |v, _| v.panels.rail_width), None, "auto");
    press_at(cx, w, gpui_kit::point(px(w0 - 1.), px(400.)));
    drag_to(cx, w, w0 + 2.);
    drag_to(cx, w, w0 + 9.);
    drag_to(cx, w, w0 + 19.);
    assert_eq!(panel(cx, &view, true).0, w0 + 20.);
    release_at(cx, w, gpui_kit::point(px(w0 + 19.), px(400.)));
    let (s, t, moving) = panel(cx, &view, true);
    assert!(moving && t == w0 && (s - (w0 + 20.)).abs() < 0.5, "it springs from where it was let go ({s} → {t})");
    let (ms, widths) = flight(cx, w, &view, true);
    assert!(widths.windows(2).all(|p| p[1] <= p[0] + 1e-3 && p[1] >= w0 - 1e-3), "{widths:?}");
    assert!((150..=260).contains(&ms), "{ms} ms on `snap`");
    assert_eq!(panel(cx, &view, true), (w0, w0, false));
    assert_eq!(view.read_with(cx, |v, _| v.panels.rail_width), None, "the auto width's detent: auto again");
    // The fold threshold is a detent too: let go just over it, it folds.
    press_at(cx, w, gpui_kit::point(px(w0 - 1.), px(400.)));
    let lo = f32::from(crate::theme::RAIL_WIDTHS.0);
    drag_to(cx, w, w0 - 4.);
    drag_to(cx, w, lo * 0.6 + 10.);
    release_at(cx, w, gpui_kit::point(px(lo * 0.6 + 10.), px(400.)));
    assert_eq!(panel(cx, &view, true).1, crate::app::STRIP, "folded");
    // Folded keeping the width it had when the drag began (Shady M1: not
    // the limit the drag clamped it to on the way).
    assert_eq!(view.read_with(cx, |v, _| v.panels.rail_width), None, "still auto");
    fly(cx, w, &view, 400, |_| {});
    view.update(cx, |v, cx| v.toggle_panel(true, cx));
    cx.run_until_parked();
    fly(cx, w, &view, 400, |_| {});
    // A width of its own, then a double-click on the edge: auto, on `sheet`.
    let w1 = panel(cx, &view, true).0;
    assert_eq!(w1, w0, "it opens at the width it had");
    press_at(cx, w, gpui_kit::point(px(w1 - 1.), px(400.)));
    drag_to(cx, w, w1 + 2.);
    drag_to(cx, w, w0 + 99.);
    release_at(cx, w, gpui_kit::point(px(w0 + 99.), px(400.)));
    assert_eq!(panel(cx, &view, true), (w0 + 100., w0 + 100., false));
    let at = gpui_kit::point(px(w0 + 99.), px(400.));
    let mut vcx = VisualTestContext::from_window(w, cx);
    for n in [1, 2] {
        vcx.simulate_event(gpui_kit::MouseDownEvent {
            button: gpui_kit::MouseButton::Left,
            position: at,
            modifiers: Default::default(),
            click_count: n,
            first_mouse: false,
        });
        vcx.simulate_event(gpui_kit::MouseUpEvent {
            button: gpui_kit::MouseButton::Left,
            position: at,
            modifiers: Default::default(),
            click_count: n,
        });
    }
    vcx.run_until_parked();
    let (s, t, moving) = panel(cx, &view, true);
    assert!(moving && t == w0 && s > w0 + 99., "a double-click springs back to the auto width ({s} → {t})");
    let (ms, _) = flight(cx, w, &view, true);
    assert!((350..=520).contains(&ms), "{ms} ms on `sheet`");
    assert_eq!(view.read_with(cx, |v, _| v.panels.rail_width), None);
}

/// fluid.md §3.8: a window resize is the window manager's: the panels
/// take their new widths at once, mid-flight or not.
#[gpui_kit::test]
fn window_resize_never_animates(cx: &mut TestAppContext) {
    let (w, view, _events) = open_at(cx, (2400., 900.), crate::theme::Panels::default().text_size);
    set_motion(cx, &view, crate::motion::Level::Full);
    settle(cx, w, 8);
    // A draft that wraps to more lines as the window narrows: the
    // composer (and the dock) take their new heights at once too (Shady
    // L3, PR #21).
    let draft = "a draft that wraps onto more lines as the window narrows and fewer as it widens, ".repeat(2);
    cx.update_window(w, |_, window, cx| view.update(cx, |v, cx| v.composer.update(cx, |s, cx| s.set_value(&draft, window, cx))))
        .expect("the window");
    cx.run_until_parked();
    fly(cx, w, &view, 400, |_| {});
    let (w0, _, _) = panel(cx, &view, true);
    let mut heights = Vec::new();
    for width in [1300., 2600., 900., 2400.] {
        VisualTestContext::from_window(w, cx).simulate_resize(size(px(width), px(900.)));
        cx.run_until_parked();
        for rail in [true, false] {
            let (s, t, moving) = panel(cx, &view, rail);
            assert!(s == t && !moving, "{width} {rail}: at once ({s} {t})");
        }
        assert!(!motion_pending(cx, &view).0, "{width}: no frame");
        heights.push(drawn(cx, w, "composer-bar".into()).expect("the composer")[3]);
    }
    assert_eq!(panel(cx, &view, true).0, w0);
    assert!(heights.iter().any(|h| *h != heights[0]), "the draft rewrapped: {heights:?}");
}

/// polish.md P13: the dock (the NEEDS YOU stub and the chips) grows and
/// shrinks on `sheet`, upward (its foot on the composer stays put),
/// clipped; then rests. Reduced: in one frame.
#[gpui_kit::test]
fn dock_height_springs_on_sheet(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    settle(cx, w, 8);
    let dock = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.dock_h.get());
    let before = drawn(cx, w, "dock-box".into()).expect("scout's chip in the dock");
    let natural = dock(cx);
    assert!((before[3] - natural).abs() < 0.5, "at rest, the dock is its height");
    // The card scrolled away: its stub comes.
    view.update(cx, |v, cx| {
        v.list.scroll_to(gpui_kit::ListOffset { item_ix: 0, offset_in_item: px(0.) });
        cx.notify();
    });
    cx.run_until_parked();
    // Its frames as laid out (the clock still: the spring at its start).
    settle(cx, w, 8);
    // Not following: the top row's top never moves (the list's view ends
    // higher each frame; its rows keep their place).
    let top_row = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.list.bounds_for_item(0).map(|b| b.top()));
    let top0 = top_row(cx);
    assert!(top0.is_some() && view.read_with(cx, |v, _| !v.list.is_following_tail()));
    let mut heights = Vec::new();
    let mut edges = 0;
    let frames = fly(cx, w, &view, 400, |cx| {
        let comp = drawn(cx, w, "composer-bar".into()).expect("the composer");
        let b = drawn(cx, w, "dock-box".into()).expect("the dock");
        heights.push((b[1], comp[1]));
        assert_eq!(top_row(cx), top0, "the top row stays put, frame {}", heights.len());
        // The gate's S1 (PR #21): while the box cuts into the stub, the
        // stub's frame leads it: its top line at the box's top.
        if dock(cx) - b[3] > crate::fit::STUB_GAP && b[3] >= 1. {
            let e = drawn(cx, w, "dock-edge".into()).expect("the stub's top edge leads");
            assert!((e[1] - b[1]).abs() < 0.5 && (e[3] - 1.).abs() < 1e-3, "at the box's top: {e:?} {b:?}");
            edges += 1;
        }
    });
    let grown = dock(cx);
    assert!(grown > natural + 20., "the stub came ({natural} → {grown})");
    assert!(frames > 30, "{frames} frames");
    assert!(edges > 5, "it led with its edge ({edges} frames)");
    // Its content's foot sits on the composer; its top rises.
    let comp = heights.last().map(|h| h.1).expect("frames");
    assert!(heights.iter().all(|h| (h.1 - comp).abs() < 0.5), "the composer stays put");
    let moved = heights.windows(2).filter(|p| p[1].0 < p[0].0 - 1e-3).count();
    assert!(moved > 20, "the dock's top rose over frames, not at once ({moved})");
    assert!(heights.windows(2).all(|p| p[1].0 <= p[0].0 + 1e-3), "and only rose");
    assert!(settle(cx, w, 8) <= 2);
    // The prompt answered elsewhere: the stub goes.
    resolve_prompt(cx, &view);
    settle(cx, w, 8);
    let top0 = top_row(cx);
    let frames = fly(cx, w, &view, 400, |cx| assert_eq!(top_row(cx), top0, "the top row stays put"));
    assert!(frames > 30, "the stub left on `sheet` ({frames})");
    assert!((dock(cx) - natural).abs() < 0.5);
    assert!(settle(cx, w, 8) <= 2);
    // Reduced: no box on a spring, the dock its height at once.
    set_motion(cx, &view, crate::motion::Level::Reduced);
    settle(cx, w, 8);
    assert!(!motion_pending(cx, &view).0, "Reduced: no frame");
    assert!(drawn(cx, w, "dock-box".into()).is_none(), "Reduced: no box on a spring");
    let b = drawn(cx, w, "dock".into()).expect("the dock");
    assert!((b[3] - natural).abs() < 0.5, "Reduced: at its height ({b:?} {natural})");
}

/// fluid.md §3.8: the composer grows a line on `flick`, upward: its
/// bottom (the caret's line) stays put; and shrinks back. Reduced: at once.
#[gpui_kit::test]
fn composer_grows_upward_on_flick(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    settle(cx, w, 8);
    let long = "a draft long enough to wrap onto a second line of the composer, and then some more words to be sure of it";
    // Following (the end stays the bottom), then scrolled back (the top
    // row stays put), every frame (Shady L4, PR #21).
    let follow = std::cell::Cell::new(true);
    let top_row = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.list.bounds_for_item(1).map(|b| b.top()));
    let mut top0 = None;
    let invariant = |cx: &mut TestAppContext, top0: Option<gpui_kit::Pixels>| {
        if follow.get() {
            let (following, end) = view.read_with(cx, |v, _| (v.list.is_following_tail(), v.list.is_scrolled_to_end()));
            assert!(following && end == Some(true), "following, at the end ({end:?})");
        } else {
            assert_eq!(top_row(cx), top0, "the top row stays put");
        }
    };
    for (text, grows) in [(long, true), ("short", false), (long, true), ("short", false)] {
        let before = drawn(cx, w, "composer-bar".into()).expect("the composer");
        cx.update_window(w, |_, window, cx| view.update(cx, |v, cx| v.composer.update(cx, |s, cx| s.set_value(text, window, cx))))
            .expect("the window");
        cx.run_until_parked();
        invariant(cx, top0);
        let mut bars = vec![drawn(cx, w, "composer-bar".into()).expect("the composer")];
        // Its new height is measured as it is laid out: the frame after.
        for _ in 0..60 {
            fluid_frame(cx, w, &view);
            invariant(cx, top0);
            bars.push(drawn(cx, w, "composer-bar".into()).expect("the composer"));
        }
        let after = *bars.last().expect("frames");
        let frames = bars.windows(2).filter(|p| p[0] != p[1]).count();
        let line = view.read_with(cx, |_, cx| crate::theme::metrics(cx).body_line);
        assert!((before[3] - after[3]).abs() >= line - 0.5, "{grows}: a line ({before:?} {after:?})");
        assert!(frames >= 2, "{grows}: {frames} frames on `flick`");
        assert!(bars.iter().all(|b| (b[1] + b[3] - (before[1] + before[3])).abs() < 0.5), "{grows}: its bottom stays put");
        let between = bars.iter().filter(|b| b[3] > before[3].min(after[3]) + 0.5 && b[3] < before[3].max(after[3]) - 0.5).count();
        assert!(between >= 1, "{grows}: it travelled ({between} frames between)");
        assert!(!motion_pending(cx, &view).0);
        if follow.get() && !grows {
            // Scrolled back: the stub comes for the card; let it land.
            follow.set(false);
            view.update(cx, |v, cx| {
                v.list.scroll_to(gpui_kit::ListOffset { item_ix: 1, offset_in_item: px(0.) });
                cx.notify();
            });
            cx.run_until_parked();
            fly(cx, w, &view, 400, |_| {});
            settle(cx, w, 8);
            top0 = top_row(cx);
            assert!(top0.is_some() && view.read_with(cx, |v, _| !v.list.is_following_tail()));
        }
    }
    set_motion(cx, &view, crate::motion::Level::Reduced);
    settle(cx, w, 8);
    let before = drawn(cx, w, "composer-bar".into()).expect("the composer");
    cx.update_window(w, |_, window, cx| view.update(cx, |v, cx| v.composer.update(cx, |s, cx| s.set_value(long, window, cx))))
        .expect("the window");
    cx.run_until_parked();
    let after = drawn(cx, w, "composer-bar".into()).expect("the composer");
    assert!(after[3] > before[3] + 10., "Reduced: grown in its frame");
    assert!(!motion_pending(cx, &view).0, "Reduced: no frame");
}

/// fluid.md §3.7: a rail section unfolding reveals its body on `sheet`
/// (what follows rides it down); folding, what follows slides up to its
/// place. Then nothing moves. Reduced: in one frame.
#[gpui_kit::test]
fn rail_section_folds_on_sheet(cx: &mut TestAppContext) {
    // The whole rail on screen: what folds is measured where it is drawn.
    let (w, view, _events) = open_at(cx, (1440., 2400.), crate::theme::Panels::default().text_size);
    set_motion(cx, &view, crate::motion::Level::Full);
    settle(cx, w, 8);
    // An open bucket with rows under it.
    let bucket = view.read_with(cx, |v, _| {
        let items = &v.rail.rail.items;
        items.windows(2).find_map(|p| match p {
            [crate::rail::Item::Bucket { bucket, open: true, .. }, crate::rail::Item::Hist(_)] => Some(*bucket),
            _ => None,
        })
    });
    let b = bucket.expect("an open HISTORY bucket in the demo");
    for open in [false, true] {
        view.update(cx, |v, cx| v.toggle_bucket(b, cx));
        cx.run_until_parked();
        let mut os = Vec::new();
        let frames = fly(cx, w, &view, 400, |cx| {
            if let Some(f) = view.read_with(cx, |v, _| v.rail_fx.fold.clone()) {
                os.push(f.o);
                if open {
                    let shown: f32 = (f.start + 1..f.after).filter_map(|ix| f.shown(ix)).map(|s| s.0).sum();
                    let total: f32 = f.heights.iter().sum();
                    assert!((shown - (total + f.o)).abs() < 0.5, "the body shows down to what follows ({shown} {total} {})", f.o);
                }
            }
        });
        assert!(frames > 30 && os.len() > 30, "{open}: {frames} frames, {} on the fold", os.len());
        assert!(if open { os[0] < -20. } else { os[0] > 20. }, "{open}: it starts where the rows were ({})", os[0]);
        assert!(os.windows(2).all(|p| p[1].abs() <= p[0].abs() + 1e-3), "{open}: toward 0 only");
        assert!(view.read_with(cx, |v, _| v.rail_fx.fold.is_none()));
        assert!(!motion_pending(cx, &view).0);
    }
    set_motion(cx, &view, crate::motion::Level::Reduced);
    settle(cx, w, 8);
    view.update(cx, |v, cx| v.toggle_bucket(b, cx));
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| v.rail_fx.fold.is_none()), "Reduced: at once");
    assert!(!motion_pending(cx, &view).0);
}

/// A section unfolding with more of the rail below than the view shows:
/// what follows rides down from where it was, the items whose place is
/// below the view included (the list draws only what is in view where it
/// lays out: they are drawn over it), so the rail never shows short.
#[gpui_kit::test]
fn a_section_unfolding_never_leaves_the_rail_short(cx: &mut TestAppContext) {
    let (w, view, _events) = open_at(cx, (1440., 700.), crate::theme::Panels::default().text_size);
    set_motion(cx, &view, crate::motion::Level::Full);
    settle(cx, w, 8);
    let (b, ix) = view
        .read_with(cx, |v, _| {
            let items = &v.rail.rail.items;
            items.windows(2).enumerate().find_map(|(ix, p)| match p {
                [crate::rail::Item::Bucket { bucket, open: true, .. }, crate::rail::Item::Hist(_)] => Some((*bucket, ix)),
                _ => None,
            })
        })
        .expect("an open HISTORY bucket in the demo");
    // The bucket at the top of the rail's view, folded (landed: unfolded
    // mid-fold, the unfold takes over from where the fold is drawn).
    view.update(cx, |v, cx| {
        v.rail_list.scroll_to(gpui_kit::ListOffset { item_ix: ix, offset_in_item: px(0.) });
        v.toggle_bucket(b, cx);
    });
    settle(cx, w, 8);
    fly(cx, w, &view, 400, |_| {});
    settle(cx, w, 8);
    catch_up(cx, &view);
    view.update(cx, |v, cx| v.toggle_bucket(b, cx));
    cx.run_until_parked();
    let mut tails = 0;
    fly(cx, w, &view, 400, |cx| {
        view.read_with(cx, |v, _| {
            let Some(f) = v.rail_fx.fold.as_ref().filter(|f| f.o < 0.) else { return };
            let vh = f32::from(v.rail_list.viewport_bounds().size.height);
            let tail = &v.rail_fx.tail;
            let h = |ix: usize| v.rail_list.bounds_for_item(ix).map(|b| f32::from(b.size.height)).unwrap_or(0.);
            if tails == 0 {
                assert!(!tail.is_empty(), "the first frame: rows below the view's end ride into it");
            }
            if let Some(&(last, top)) = tail.last() {
                tails += 1;
                assert!(top + h(last) >= vh - 0.5 || last + 1 == v.rail.rail.items.len(), "down to the view's end ({top} {vh})");
                for p in tail.windows(2) {
                    assert!((p[1].1 - (p[0].1 + h(p[0].0))).abs() < 0.5 && p[1].0 == p[0].0 + 1, "contiguous: {p:?}");
                }
                let first = tail[0];
                let at = v.rail_list.bounds_for_item(first.0).map(|b| f32::from(b.top() - v.rail_list.viewport_bounds().top()));
                assert!(at.is_some_and(|y| y >= vh - 0.5 && (y + f.o - first.1).abs() < 0.5), "its place, ridden: {at:?} {first:?}");
            }
        })
    });
    assert!(tails > 3, "{tails}");
    assert!(view.read_with(cx, |v, _| v.rail_fx.fold.is_none() && v.rail_fx.tail.is_empty()));
}

/// P13: a chip that comes rises 8 px on `flick` and lifts from 70 %; a
/// chip that leaves keeps its slot while it fades, then the line closes.
#[test]
fn a_chip_arrives_from_below_at_lift() {
    use crate::motion::{ARRIVE, LIFT, Level, Motion};
    let t0 = std::time::Instant::now();
    let fx = Motion::new(t0);
    fx.begin_frame(Level::Full, false, t0);
    assert_eq!(fx.arrival(Some(t0 + std::time::Duration::from_millis(18))), (ARRIVE, LIFT), "staggered: still to come");
    let mut last = (ARRIVE + 1., 0.);
    for ms in (0..400).step_by(7) {
        fx.begin_frame(Level::Full, false, t0 + std::time::Duration::from_millis(ms));
        let (travel, strength) = fx.arrival(Some(t0));
        assert!(travel <= last.0 && strength >= last.1 && strength >= LIFT, "{ms}: {travel} {strength}");
        last = (travel, strength);
    }
    assert_eq!(last, (0., 1.), "at rest");
    fx.begin_frame(Level::Reduced, false, t0);
    assert_eq!(fx.arrival(Some(t0)), (0., 1.), "Reduced: no travel");
}

/// Shady H1, the gate's must-fix 1 (PR #21): a panel flies on whole
/// pixels, so the transcript list's item width is the same in every frame
/// of a fold or an unfold, and GPUI keeps every measured row height. At
/// 1920 (the column capped: nothing rewraps) the scroll range and offset,
/// so the scrollbar, are the same before, in and after the flight of
/// either panel, scrolled back (no follow to re-end it).
#[gpui_kit::test]
fn a_fold_keeps_the_transcripts_measured_heights(cx: &mut TestAppContext) {
    let (w, view, _events) = open_at(cx, (1920., 700.), crate::theme::Panels::default().text_size);
    set_motion(cx, &view, crate::motion::Level::Full);
    settle(cx, w, 8);
    view.update(cx, |v, _| v.list.scroll_to(gpui_kit::ListOffset { item_ix: 2, offset_in_item: px(13.) }));
    frame(cx, &view);
    // The card scrolled away: its stub comes (the view shortens); landed.
    settle(cx, w, 8);
    catch_up(cx, &view);
    fly(cx, w, &view, 400, |_| {});
    settle(cx, w, 8);
    let geo = |cx: &mut TestAppContext| {
        view.read_with(cx, |v, _| {
            let item = v.list.bounds_for_item(2).map(|b| f32::from(b.size.width));
            (v.list.max_offset_for_scrollbar().y, v.list.scroll_px_offset_for_scrollbar().y, item, v.layout.column)
        })
    };
    let before = geo(cx);
    assert!(before.0 > px(0.) && before.2 == Some(before.3), "scrolled back, scrollable: {before:?}");
    for rail in [true, false] {
        for _ in 0..2 {
            catch_up(cx, &view);
            view.update(cx, |v, cx| v.toggle_panel(rail, cx));
            cx.run_until_parked();
            assert_eq!(geo(cx), before, "{rail}: its first frame");
            let frames = fly(cx, w, &view, 400, |cx| assert_eq!(geo(cx), before, "{rail}: in flight"));
            assert!(frames > 30, "{rail}: it flew ({frames})");
            settle(cx, w, 8);
            assert_eq!(geo(cx), before, "{rail}: landed");
        }
    }
}

/// Shady M2 (PR #21): a panel's edge takes no press while the panel flies
/// (a drag caught in flight would start from where it is going, not where
/// it is drawn); landed, it is back. A drag of one panel's edge leaves the
/// other's flight flying, from where it is.
#[gpui_kit::test]
fn an_edge_waits_for_its_panel_to_land(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    settle(cx, w, 8);
    assert!(drawn(cx, w, "rail-edge".into()).is_some(), "at rest: the edge");
    view.update(cx, |v, cx| v.toggle_panel(true, cx));
    cx.run_until_parked();
    for _ in 0..10 {
        fluid_frame(cx, w, &view);
    }
    let (s, t, moving) = panel(cx, &view, true);
    assert!(moving && s > t + 50., "in flight ({s} {t})");
    assert!(drawn(cx, w, "rail-edge".into()).is_none(), "in flight: no edge");
    press_at(cx, w, gpui_kit::point(px(s - 1.), px(400.)));
    drag_to(cx, w, s + 40.);
    assert!(view.read_with(cx, |v, _| v.drag.is_none()), "no drag of the edge");
    release_at(cx, w, gpui_kit::point(px(s + 40.), px(400.)));
    fly(cx, w, &view, 400, |_| {});
    assert_eq!(panel(cx, &view, true), (t, t, false), "it landed where it was going");
    assert!(drawn(cx, w, "rail-edge".into()).is_some(), "landed: the strip's edge");
    // The rail opens again; the right panel folds, and while it flies the
    // rail's edge is dragged: the right panel flies on, one way, to its strip.
    view.update(cx, |v, cx| v.toggle_panel(true, cx));
    cx.run_until_parked();
    fly(cx, w, &view, 400, |_| {});
    let (r0, _, _) = panel(cx, &view, true);
    view.update(cx, |v, cx| v.toggle_panel(false, cx));
    cx.run_until_parked();
    for _ in 0..8 {
        fluid_frame(cx, w, &view);
    }
    let mut last = panel(cx, &view, false).0;
    press_at(cx, w, gpui_kit::point(px(r0 - 1.), px(400.)));
    for k in 1..=6 {
        drag_to(cx, w, r0 + 2. + 5. * k as f32);
        fluid_frame(cx, w, &view);
        let (s, t, moving) = panel(cx, &view, false);
        assert!(s <= last + 1e-3, "the right panel flies on to its strip ({last} → {s})");
        assert!(moving || s == t, "{s} {t}");
        last = s;
    }
    assert!(panel(cx, &view, false).2, "still flying (not snapped to its target)");
    release_at(cx, w, gpui_kit::point(px(r0 + 31.), px(400.)));
    fly(cx, w, &view, 400, |_| {});
    assert_eq!(panel(cx, &view, false), (crate::app::STRIP, crate::app::STRIP, false));
}

/// The gate's must-fix 2 and 3 (PR #21): a section mid-rail, the rail
/// scrolled to its end. Folding: from its first frame its body is drawn
/// under its head down to what follows as that rises over it (no empty
/// band opens), and the rows above it never jump (the rail keeps its
/// length until the fold lands: its offset gives way with the fold, never
/// in one frame). Unfolding: from its first frame the rows after it are
/// drawn whole down to the view's end (the list's, then the ridden tail).
#[gpui_kit::test]
fn a_section_folds_and_unfolds_mid_rail_in_a_scrolled_rail(cx: &mut TestAppContext) {
    let (w, view, _events) = open_at(cx, (1280., 720.), crate::theme::Panels::default().text_size);
    set_motion(cx, &view, crate::motion::Level::Full);
    settle(cx, w, 8);
    // The last open bucket with rows under it (in view at the rail's end),
    // past the rail's first item.
    let (b, ix) = view
        .read_with(cx, |v, _| {
            let items = &v.rail.rail.items;
            items.windows(2).enumerate().skip(1).collect::<Vec<_>>().into_iter().rev().find_map(|(ix, p)| match p {
                [crate::rail::Item::Bucket { bucket, open: true, .. }, crate::rail::Item::Hist(_)] => Some((*bucket, ix)),
                _ => None,
            })
        })
        .expect("an open HISTORY bucket in the demo");
    let to_end = |cx: &mut TestAppContext| {
        view.update(cx, |v, cx| {
            let n = v.rail.rail.items.len();
            v.rail_list.scroll_to(gpui_kit::ListOffset { item_ix: n, offset_in_item: px(0.) });
            cx.notify();
        });
        cx.run_until_parked();
        settle(cx, w, 8);
    };
    to_end(cx);
    let (scrolled, head0) = view.read_with(cx, |v, _| {
        let vp = v.rail_list.viewport_bounds();
        (-f32::from(v.rail_list.scroll_px_offset_for_scrollbar().y), v.rail_list.bounds_for_item(ix).map(|r| f32::from(r.top() - vp.top())))
    });
    assert!(scrolled > 0.5 && head0.is_some(), "the rail scrolled ({scrolled}), the bucket in view ({head0:?})");
    // Where an item is in the view, as laid out this frame.
    let place = |v: &AppView, ix: usize| {
        let vp = v.rail_list.viewport_bounds();
        v.rail_list.bounds_for_item(ix).map(|r| (f32::from(r.top() - vp.top()), f32::from(r.size.height)))
    };
    // Fold.
    catch_up(cx, &view);
    view.update(cx, |v, cx| v.toggle_bucket(b, cx));
    cx.run_until_parked();
    let mut heads = vec![head0.expect("the head")];
    let mut ghosts = 0;
    let mut check = |cx: &mut TestAppContext| {
        view.read_with(cx, |v, _| {
            let head = place(v, ix).expect("the head in view").0;
            heads.push(head);
            let Some(f) = v.rail_fx.fold.as_ref().filter(|f| f.o >= 1.) else { return };
            let (y, h, n) = v.rail_fx.ghost_at.expect("its body drawn under its head");
            let (ht, hh) = place(v, f.start).expect("the head");
            assert!(
                (y - (ht + hh)).abs() < 0.5 && (h - f.o).abs() < 1e-3,
                "under the head, down to what follows: {y} {h} {ht}+{hh} {}",
                f.o
            );
            assert!(n > 0 && f.after == f.start + 1, "{n} {f:?}");
            if let Some((at, _)) = place(v, f.after) {
                assert!((at - (ht + hh)).abs() < 0.5, "what follows rides from the head's foot: {at}");
            }
            ghosts += 1;
        })
    };
    check(cx);
    let frames = fly(cx, w, &view, 400, &mut check);
    assert!(frames > 30 && ghosts > 30, "{frames} frames, {ghosts} with the body drawn");
    assert_eq!(heads[1], heads[0], "the rows above don't move in the fold's first frame");
    assert!(heads.windows(2).all(|p| (p[1] - p[0]).abs() <= 13.), "the rows above glide, never jump: {heads:?}");
    assert!(view.read_with(cx, |v, _| v.rail_fx.fold.is_none() && v.rail_fx.ghost_at.is_none()));
    // Unfold, scrolled to the end again.
    to_end(cx);
    catch_up(cx, &view);
    view.update(cx, |v, cx| v.toggle_bucket(b, cx));
    cx.run_until_parked();
    let mut tails = 0;
    let mut first = true;
    let mut cover = |cx: &mut TestAppContext| {
        view.read_with(cx, |v, _| {
            let Some(f) = v.rail_fx.fold.as_ref().filter(|f| f.o < 0.) else { return };
            let vh = f32::from(v.rail_list.viewport_bounds().size.height);
            let n = v.rail.rail.items.len();
            // The list draws the items whose place is in the view, `o` up.
            let mut end = None;
            let mut next = f.after;
            while next < n {
                let Some((t, h)) = place(v, next) else { break };
                if t >= vh {
                    break;
                }
                end = Some(t + h + f.o);
                next += 1;
            }
            // What follows starts below the view: the body revealed fills it.
            if place(v, f.after).is_some_and(|(t, _)| t + f.o >= vh - 0.5) {
                first = false;
                return;
            }
            // Then the tail, contiguous, from the next one.
            for &(tix, top) in &v.rail_fx.tail {
                assert_eq!(tix, next, "the tail goes on from the list's last: {:?}", v.rail_fx.tail);
                if let Some(e) = end {
                    assert!((top - e).abs() < 0.5, "contiguous: {top} after {e}");
                }
                end = Some(top + place(v, tix).expect("laid out").1);
                next += 1;
                tails += 1;
            }
            assert!(
                next == n || end.is_some_and(|e| e >= vh - 0.5),
                "whole down to the view's end{}: {end:?} {vh}",
                if first { " (first frame)" } else { "" }
            );
            first = false;
        })
    };
    cover(cx);
    let frames = fly(cx, w, &view, 400, &mut cover);
    assert!(frames > 30 && tails > 3, "{frames} frames, {tails} tail rows");
    assert!(!first, "checked from its first frame");
    assert!(view.read_with(cx, |v, _| v.rail_fx.fold.is_none() && v.rail_fx.tail.is_empty()));
}

/// Haseeb's chips (Q5 r2): a chip is the column's share, whatever it says.
/// A line holds as many as are `MIN_CHIP` wide (two in the 1280-px window
/// with both panels open), each as wide as the others, `CHIP_GAP` apart,
/// filling the column. A new step, its clock, the hover's ×, an armed
/// cancel or a state's change moves no chip and leaves the dock its
/// height. A panel in flight leaves them be (the frozen column); landed,
/// they take the new column's share.
#[gpui_kit::test]
fn a_chip_is_the_columns_share_whatever_it_says(cx: &mut TestAppContext) {
    use gpui_kit::{Modifiers, MouseMoveEvent};
    let (w, view, _events) = open_at(cx, (1280., 800.), 16);
    set_motion(cx, &view, crate::motion::Level::Full);
    // The demo's session attached (its rows in) before these replace them.
    for _ in 0..50 {
        if view.read_with(cx, |v, _| v.model.stats.subagents.ident(1).is_some()) {
            break;
        }
        settle(cx, w, 2);
    }
    assert!(view.read_with(cx, |v, _| v.model.stats.subagents.ident(1).is_some()), "the demo attached");
    // The demo's own workers (1, 2) ended: these are 11–13.
    let names = ["scout", "flake-statistics-collector", "bisect"];
    let rows = |step: &str, secs: f64, third: &str| {
        let rows: Vec<serde_json::Value> = (11..=13u64)
            .map(|n| {
                let status = if n == 13 { third } else { "running" };
                let done = (status != "running").then_some(secs);
                serde_json::json!({"subagent_id": n, "agent_name": names[n as usize - 11], "status": status, "cancel_requested": false,
                    "elapsed_secs": secs, "finished_elapsed": done, "step": step, "tools": n, "handle_id": format!("sa_{n}")})
            })
            .collect();
        synaps_client::protocol::decode_event(&serde_json::json!({"ev": "subagent_rows", "rows": rows})).expect("a rows event")
    };
    let set = |cx: &mut TestAppContext, ev: synaps_client::protocol::SessionEvent| {
        view.update(cx, |v, cx| {
            v.model.stats.subagents.on_session(&ev);
            cx.notify();
        });
        settle(cx, w, 8);
        catch_up(cx, &view);
        // Long enough for an arrival (not for an ended one's flash to end:
        // the clocks keep frames coming).
        for _ in 0..72 {
            fluid_frame(cx, w, &view);
        }
        settle(cx, w, 8);
    };
    let geo = |cx: &mut TestAppContext| {
        let chips: Vec<[f32; 4]> =
            (11..=13).map(|id| drawn(cx, w, format!("chip {id}")).unwrap_or_else(|| panic!("chip {id} drawn"))).collect();
        (chips, drawn(cx, w, "dock-box".into()))
    };
    set(cx, rows("ls", 2., "running"));
    let before = geo(cx);
    let (k, cw, column) = view.read_with(cx, |v, _| {
        let (k, cw) = crate::tray::grid(v.layout.column, &crate::theme::Metrics::at(16));
        (k, cw, v.layout.column)
    });
    assert_eq!(k, 2, "two a line at 1280 with both panels open ({column})");
    // In place order (the newest running first).
    let mut placed = before.0.clone();
    placed.sort_by(|p, q| (p[1], p[0]).partial_cmp(&(q[1], q[0])).expect("numbers"));
    let [a, b, c] = [placed[0], placed[1], placed[2]];
    assert!(a[2] == cw && b[2] == cw && c[2] == cw, "every chip the line's share: {:?}", before.0);
    assert_eq!((a[1], b[0] - (a[0] + a[2])), (b[1], crate::tray::CHIP_GAP), "a line's two, 8 apart");
    assert!(column - (b[0] + b[2] - a[0]) < 2., "they fill the column ({column}): {a:?} {b:?}");
    assert!(c[1] > a[1] && c[0] == a[0], "the third starts the next line: {c:?}");
    let same = |cx: &mut TestAppContext, what: &str| assert_eq!(geo(cx), before, "{what}");
    set(cx, rows("cargo test -p relay --test refresh_race -- --nocapture and then some", 2., "running"));
    same(cx, "a long step");
    set(cx, rows("git status", 3599., "running"));
    same(cx, "a clock");
    let at = VisualTestContext::from_window(w, cx).debug_bounds("chip 11").expect("chip 11").center();
    event(cx, w, PlatformInput::MouseMove(MouseMoveEvent { position: at, pressed_button: None, modifiers: Modifiers::default() }));
    settle(cx, w, 8);
    same(cx, "hovered (its ×)");
    let who = view.read_with(cx, |v, _| {
        let (call, name) = v.model.stats.subagents.ident(11).expect("sa_11 named");
        crate::wtabs::Who::new(11, name.to_string(), call.map(|c| c.to_string().into()))
    });
    view.update(cx, |v, cx| v.press_cancel(crate::demo::SID, &who, crate::menu::How::Click(1), cx));
    settle(cx, w, 8);
    assert!(drawn(cx, w, "tray-confirm".into()).is_some(), "armed: CONFIRM");
    same(cx, "armed");
    // A panel in flight: the frozen column's share; landed, the new one's.
    catch_up(cx, &view);
    view.update(cx, |v, cx| v.toggle_panel(true, cx));
    cx.run_until_parked();
    let mut frames = 0;
    while view.read_with(cx, |v, _| v.panel_fx.moving()) && frames < 400 {
        assert_eq!(geo(cx).0.iter().map(|c| c[2]).collect::<Vec<_>>(), [cw; 3], "in flight");
        fluid_frame(cx, w, &view);
        frames += 1;
    }
    assert!(frames > 30, "it flew ({frames} frames)");
    settle(cx, w, 8);
    let now = view.read_with(cx, |v, _| crate::tray::grid(v.layout.column, &crate::theme::Metrics::at(16)).1);
    let landed = geo(cx);
    assert!(now > cw && landed.0.iter().all(|c| c[2] == now), "landed: the wider column's share ({now})");
    set(cx, rows("git status", 3600., "failed"));
    assert_eq!(geo(cx), landed, "the third failed");
}

/// Shady L2 (PR #21): another splice of the rail while a section folds
/// (a session favourited: its row moves) ends the fold, so no rows are
/// offset by a fold whose indices are no longer theirs.
#[gpui_kit::test]
fn a_rail_splice_mid_fold_ends_the_fold(cx: &mut TestAppContext) {
    let (w, view, _events) = open_at(cx, (1280., 720.), crate::theme::Panels::default().text_size);
    set_motion(cx, &view, crate::motion::Level::Full);
    settle(cx, w, 8);
    let b = view
        .read_with(cx, |v, _| {
            v.rail.rail.items.iter().find_map(|i| match i {
                crate::rail::Item::Bucket { bucket, open: true, .. } => Some(*bucket),
                _ => None,
            })
        })
        .expect("an open bucket");
    catch_up(cx, &view);
    view.update(cx, |v, cx| v.toggle_bucket(b, cx));
    cx.run_until_parked();
    fluid_frame(cx, w, &view);
    fluid_frame(cx, w, &view);
    assert!(view.read_with(cx, |v, _| v.rail_fx.fold.is_some()), "folding");
    view.update(cx, |v, cx| {
        v.toggle_fav_session(crate::demo::SID);
        cx.notify();
    });
    cx.run_until_parked();
    fluid_frame(cx, w, &view);
    assert!(view.read_with(cx, |v, _| v.rail_fx.fold.is_none()), "the splice ended the fold");
    fly(cx, w, &view, 400, |_| {});
    settle(cx, w, 8);
    assert!(view.read_with(cx, |v, _| v.rail_fx.fold.is_none() && v.rail_fx.offsets.is_empty()), "at rest: nothing offset");
}

/// Shady R2-H1, the gate's must-fix B (PR #21): a chip whose cell changes
/// (a worker ends and sorts after the running ones) slides there in px,
/// x and y from one instant, the whole way: every 144 Hz frame it is
/// between the two cells and goes one way, no frame moves it more than a
/// spring's step, and it lands on its cell exactly. A moving chip is on
/// the canvas (what it passes is under it).
#[gpui_kit::test]
fn a_chip_slides_to_its_new_cell_the_whole_way(cx: &mut TestAppContext) {
    let (w, view, _events) = open_at(cx, (1280., 800.), 16);
    set_motion(cx, &view, crate::motion::Level::Full);
    for _ in 0..50 {
        if view.read_with(cx, |v, _| v.model.stats.subagents.ident(1).is_some()) {
            break;
        }
        settle(cx, w, 2);
    }
    let names = ["scout", "reviewer", "trace-cart"];
    let rows = |s13: &str, s12: &str| {
        let rows: Vec<serde_json::Value> = (11..=13u64)
            .map(|n| {
                let status = match n {
                    13 => s13,
                    12 => s12,
                    _ => "running",
                };
                let done = (status != "running").then_some(9.);
                serde_json::json!({"subagent_id": n, "agent_name": names[n as usize - 11], "status": status, "cancel_requested": false,
                    "elapsed_secs": 9., "finished_elapsed": done, "step": "ls", "tools": n, "handle_id": format!("sa_{n}")})
            })
            .collect();
        synaps_client::protocol::decode_event(&serde_json::json!({"ev": "subagent_rows", "rows": rows})).expect("a rows event")
    };
    let set = |cx: &mut TestAppContext, ev: synaps_client::protocol::SessionEvent| {
        view.update(cx, |v, cx| {
            v.model.stats.subagents.on_session(&ev);
            cx.notify();
        });
    };
    let geo = |cx: &mut TestAppContext| -> Vec<[f32; 4]> {
        (11..=13).map(|id| drawn(cx, w, format!("chip {id}")).unwrap_or_else(|| panic!("chip {id} drawn"))).collect()
    };
    set(cx, rows("running", "running"));
    settle(cx, w, 8);
    catch_up(cx, &view);
    for _ in 0..72 {
        fluid_frame(cx, w, &view);
    }
    settle(cx, w, 8);
    // Newest running first: 13, 12 on the first line, 11 under 13.
    let before = geo(cx);
    let pitch = view.read_with(cx, |v, _| crate::tray::grid(v.layout.column, &crate::theme::Metrics::at(16)).1) + crate::tray::CHIP_GAP;
    assert!(before[2][0] < before[1][0] && before[2][1] == before[1][1] && before[0][1] > before[2][1], "{before:?}");
    // 13 ends: it sorts after the running ones. 12 goes left, 11 up and
    // right, 13 down a line (after the demo's own ended workers' ghosts'
    // cells, if any).
    catch_up(cx, &view);
    set(cx, rows("completed", "running"));
    cx.run_until_parked();
    let mut path = vec![before.clone()];
    for _ in 0..140 {
        fluid_frame(cx, w, &view);
        path.push(geo(cx));
    }
    settle(cx, w, 8);
    let after = geo(cx);
    assert_eq!(after.last(), path.last().and_then(|p| p.last()), "at rest by the end");
    assert_eq!((after[1][0], after[1][1]), (before[2][0], before[2][1]), "12 took 13's cell");
    assert_eq!((after[0][0], after[0][1]), (before[1][0], before[1][1]), "11 took 12's");
    assert!(after[2][0] != before[2][0] && after[2][1] != before[2][1], "13 went across and down: {before:?} {after:?}");
    for (c, id) in (11..=13).enumerate() {
        for axis in 0..2 {
            let (a, b) = (before[c][axis], after[c][axis]);
            let xs: Vec<f32> = path.iter().map(|p| p[c][axis]).collect();
            let steps: Vec<f32> = xs.windows(2).map(|p| p[1] - p[0]).collect();
            let (lo, hi) = (a.min(b), a.max(b));
            assert!(xs.iter().all(|x| (lo..=hi).contains(x)), "chip {id} axis {axis}: between its cells {a} → {b}: {xs:?}");
            assert!(steps.iter().all(|d| d * (b - a) >= 0.), "chip {id} axis {axis}: one way: {xs:?}");
            let most = steps.iter().fold(0f32, |m, d| m.max(d.abs()));
            assert!(most < pitch / 4., "chip {id} axis {axis}: a frame moved {most} px (pitch {pitch}): {xs:?}");
            if a != b {
                let moving = steps.iter().filter(|d| **d != 0.).count();
                assert!(moving > 20, "chip {id} axis {axis}: it slid ({moving} frames): {xs:?}");
            }
            assert_eq!(xs.last(), Some(&b), "chip {id} axis {axis}: lands exactly");
        }
    }
    // Off: in one frame (12 ends too: 11, 13, 12).
    set_motion(cx, &view, crate::motion::Level::Off);
    settle(cx, w, 8);
    set(cx, rows("completed", "completed"));
    cx.run_until_parked();
    settle(cx, w, 1);
    let cut = geo(cx);
    settle(cx, w, 8);
    assert!(cut != after && cut == geo(cx), "Off: at its cell at once: {cut:?} {after:?}");
}

/// The gate's must-fix A (PR #21): Ctrl+J lands at every level. Full
/// glides one way; Reduced and Off cut to the card in the frame after the
/// key, the stub going with the jump, never before the card shows.
#[gpui_kit::test]
fn ctrl_j_lands_at_every_level(cx: &mut TestAppContext) {
    let (w, view, _events) = open(cx);
    let has = |cx: &mut TestAppContext, sel: &'static str| VisualTestContext::from_window(w, cx).debug_bounds(sel).is_some();
    for level in [crate::motion::Level::Full, crate::motion::Level::Reduced, crate::motion::Level::Off] {
        set_motion(cx, &view, level);
        view.update(cx, |v, cx| {
            v.list.scroll_to(gpui_kit::ListOffset { item_ix: 0, offset_in_item: gpui_kit::px(0.) });
            cx.notify();
        });
        settle(cx, w, 8);
        fly(cx, w, &view, 400, |_| {});
        settle(cx, w, 8);
        assert!(view.read_with(cx, |v, _| !v.card_on_screen()) && has(cx, "needs-stub"), "{level:?}: scrolled away, the stub");
        focus_composer(cx, w, &view);
        catch_up(cx, &view);
        tap(cx, w, "ctrl-j");
        // Every frame: the stub, or the card in view (never neither).
        let in_view = |v: &AppView| {
            let (top, bottom) = v.list_view();
            v.card_ix()
                .and_then(|ix| v.list.bounds_for_item(ix))
                .is_some_and(|b| f32::from(b.top()) < bottom && f32::from(b.bottom()) > top)
        };
        let mut first = None;
        for n in 0..120 {
            fluid_frame(cx, w, &view);
            let (card, stub) = (view.read_with(cx, |v, _| in_view(v)), has(cx, "needs-stub"));
            assert!(card || stub || level == crate::motion::Level::Full, "{level:?}, frame {n}: neither the card nor the stub");
            if card && first.is_none() {
                first = Some(n);
            }
        }
        settle(cx, w, 8);
        assert!(view.read_with(cx, |v, _| v.card_on_screen()) && !has(cx, "needs-stub"), "{level:?}: in view, no stub");
        if level != crate::motion::Level::Full {
            assert_eq!(first, Some(0), "{level:?}: the card in view in the frame after the key");
        }
    }
}

/// Shady R2-M1 (PR #21): at 125, 150 and 175 % the transcript's item width
/// is the column, bit for bit, in every frame of a rail fold and unfold
/// (the pane from device pixels, as the layout rounds it): the list keeps
/// its heights, following stays at the end.
#[gpui_kit::test]
fn the_column_holds_through_a_flight_at_fractional_scales(cx: &mut TestAppContext) {
    let (w, view, _events) = open_at(cx, (1280., 720.), crate::theme::Panels::default().text_size);
    set_motion(cx, &view, crate::motion::Level::Full);
    for scale in [1.5, 1.25, 1.75] {
        VisualTestContext::from_window(w, cx).simulate_scale_factor_change(scale);
        cx.run_until_parked();
        settle(cx, w, 8);
        assert_eq!(cx.update_window(w, |_, window, _| window.scale_factor()).expect("the window"), scale);
        for _ in 0..2 {
            view.update(cx, |v, _| v.list.scroll_to_end());
            frame(cx, &view);
            settle(cx, w, 8);
            let mut last: Option<f32> = None;
            let check = |cx: &mut TestAppContext, last: &mut Option<f32>| {
                let (column, item, end) = view.read_with(cx, |v, _| {
                    let item = v.list.bounds_for_item(v.disp.len() - 1).map(|b| f32::from(b.size.width));
                    (v.layout.column, item, v.list.is_scrolled_to_end())
                });
                assert_eq!(item, Some(column), "{scale}: the list's item width is the column");
                // A new column (the landing's rewrap) measures again.
                assert!(end == Some(true) || *last != Some(column), "{scale}: every height known, at the end ({end:?})");
                *last = Some(column);
            };
            catch_up(cx, &view);
            view.update(cx, |v, cx| v.toggle_panel(true, cx));
            cx.run_until_parked();
            let frames = fly(cx, w, &view, 400, |cx| check(cx, &mut last));
            assert!(frames > 30, "{scale}: it flew ({frames})");
            settle(cx, w, 8);
        }
    }
}

/// Shady R2-M2 (PR #21): FAVOURITES folding with a favourite it keeps
/// (one that needs you, between two that go) folds as a bucket does: its
/// body as it was stays drawn under its head, down to what follows as
/// that rises over it, the one it keeps whole and rising with it once
/// reached. No empty band in any frame; landed, the kept one is under
/// the head.
#[gpui_kit::test]
fn favourites_fold_keeping_a_busy_one_leaves_no_gap(cx: &mut TestAppContext) {
    let (w, view, _events) = open_at(cx, (1280., 900.), crate::theme::Panels::default().text_size);
    let (head, body, star) = three_favourites(cx, w, &view);
    let place = |v: &AppView, ix: usize| {
        let vp = v.rail_list.viewport_bounds();
        v.rail_list.bounds_for_item(ix).map(|r| (f32::from(r.top() - vp.top()), f32::from(r.size.height)))
    };
    let star_y = |cx: &mut TestAppContext| drawn(cx, w, star.clone()).map(|b| b[1]);
    let mut stars = vec![star_y(cx).expect("its star drawn")];
    catch_up(cx, &view);
    view.update(cx, |v, cx| v.toggle_favs_group(cx));
    cx.run_until_parked();
    let mut ghosts = 0;
    let mut kept_y = Vec::new();
    let mut check = |cx: &mut TestAppContext| {
        view.read_with(cx, |v, _| {
            let Some(f) = v.rail_fx.fold.as_ref().filter(|f| f.o >= 1.) else { return };
            let g = f.ghost.as_ref().expect("a ghost: the body as it was");
            let kept = g.kept();
            assert!(kept > 0. && f.after == f.start + 2, "it keeps one: {f:?}");
            let (y, h, n) = v.rail_fx.ghost_at.expect("its body drawn under its head");
            let (ht, hh) = place(v, f.start).expect("the head");
            assert!((y - (ht + hh)).abs() < 0.5, "from the head's foot: {y} {ht}+{hh}");
            assert!((h - (kept + f.o)).abs() < 1e-3 && n == body, "down to what follows: {h} {kept} {} {n}", f.o);
            if let Some((at, _)) = place(v, f.after) {
                assert!((at + f.o - (y + h)).abs() < 0.5, "what follows starts where the ghost ends: {at}+{} vs {}", f.o, y + h);
            }
            let shown = g.shown(f.o);
            let ky: f32 = g.items.iter().zip(&shown).take_while(|(i, _)| !i.0).map(|(_, s)| s).sum();
            kept_y.push(y + ky);
            ghosts += 1;
        });
        stars.push(star_y(cx).expect("the kept one drawn in every frame"));
    };
    check(cx);
    let frames = fly(cx, w, &view, 400, &mut check);
    let most = stars.windows(2).fold(0f32, |m, p| m.max((p[1] - p[0]).abs()));
    assert!(stars.windows(2).all(|p| p[1] <= p[0] + 1e-3) && most < 8., "the kept one rises as drawn, never jumps: {stars:?}");
    assert!(frames > 30 && ghosts > 30, "{frames} frames, {ghosts} with the body drawn");
    assert!(kept_y.windows(2).all(|p| p[1] <= p[0] + 1e-3), "the kept one only rises: {kept_y:?}");
    view.read_with(cx, |v, _| {
        assert!(v.rail_fx.fold.is_none() && v.rail_fx.ghost_at.is_none(), "landed");
        let (ht, hh) = place(v, head).expect("the head");
        let (kt, _) = place(v, head + 1).expect("the kept one");
        assert!((kt - (ht + hh)).abs() < 0.5, "under the head");
        // (The last checked frame had `o` ≥ 1 left.)
        assert!((kept_y.last().copied().unwrap_or(kt) - kt).abs() < 2., "where its ghost was last drawn: {kept_y:?} {kt}");
    });
}

/// Shady R3-M1, the gate's r3 C (PR #21): FAVOURITES unfolding with the
/// favourite it kept (one that needs you, a favourite before it and one
/// after) is its fold run backwards: the kept one is drawn whole in
/// every frame, only going down as the one above it opens, then the one
/// under it. Turned round either way mid-flight (a double click), it
/// goes on from where the kept one is drawn: no step.
#[gpui_kit::test]
fn favourites_unfold_keeping_a_busy_one_draws_it_every_frame(cx: &mut TestAppContext) {
    let (w, view, _events) = open_at(cx, (1280., 900.), crate::theme::Panels::default().text_size);
    let (head, _, star) = three_favourites(cx, w, &view);
    let star_y = |cx: &mut TestAppContext| drawn(cx, w, star.clone()).map(|b| b[1]);
    let toggle = |cx: &mut TestAppContext| {
        view.update(cx, |v, cx| v.toggle_favs_group(cx));
        cx.run_until_parked();
    };
    catch_up(cx, &view);
    toggle(cx);
    fly(cx, w, &view, 400, |_| {});
    settle(cx, w, 8);
    let folded = star_y(cx).expect("folded: the kept one drawn");
    // Unfolding: whole, from under the head, down only.
    let mut stars = vec![folded];
    let mut opens = 0;
    catch_up(cx, &view);
    toggle(cx);
    let frames = fly(cx, w, &view, 400, |cx| {
        view.read_with(cx, |v, _| {
            let Some(f) = v.rail_fx.fold.as_ref().filter(|f| f.o <= -1.) else { return };
            assert!(f.after == f.start + 4 && f.kept == [false, true, false], "two open, one kept: {f:?}");
            let (s, _) = f.shown(f.start + 2).expect("in the body");
            assert_eq!(s, f.heights[1], "the kept one whole");
            let all: f32 = (f.start + 1..f.after).filter_map(|ix| f.shown(ix)).map(|s| s.0).sum();
            let total: f32 = f.heights.iter().sum();
            assert!((all - (total + f.o)).abs() < 0.5, "the body down to what follows ({all} {total} {})", f.o);
            opens += 1;
        });
        stars.push(star_y(cx).expect("the kept one drawn in every frame"));
    });
    let most = stars.windows(2).fold(0f32, |m, p| m.max((p[1] - p[0]).abs()));
    assert!(frames > 30 && opens > 30, "{frames} frames, {opens} unfolding");
    assert!(stars.windows(2).all(|p| p[1] >= p[0] - 1e-3) && most < 8., "the kept one goes down as drawn, never jumps: {stars:?}");
    assert!(stars.last().copied().unwrap_or(0.) - folded > 20., "it ends under the one above it: {stars:?}");
    settle(cx, w, 8);
    view.read_with(cx, |v, _| {
        assert!(v.rail.rail.items.get(head).is_some_and(|i| matches!(i, crate::rail::Item::Favs { open: true, .. })), "open")
    });
    // Turned round: open → folding → unfolding, then folded → unfolding
    // → folding; the kept one drawn in every frame, no step.
    for (open, turn) in [(true, 12), (true, 20), (false, 12), (false, 20)] {
        let at = view.read_with(cx, |v, _| matches!(v.rail.rail.items.get(head), Some(crate::rail::Item::Favs { open: true, .. })));
        if at != open {
            catch_up(cx, &view);
            toggle(cx);
            fly(cx, w, &view, 400, |_| {});
            settle(cx, w, 8);
        }
        let mut stars = vec![star_y(cx).expect("drawn")];
        catch_up(cx, &view);
        toggle(cx);
        for n in 0..200 {
            if n == turn {
                toggle(cx);
            }
            fluid_frame(cx, w, &view);
            stars.push(star_y(cx).expect("the kept one drawn in every frame"));
        }
        settle(cx, w, 8);
        let most = stars.windows(2).fold(0f32, |m, p| m.max((p[1] - p[0]).abs()));
        assert!(most < 8., "{open}, turned at frame {turn}: the kept one jumped {most} px: {stars:?}");
        assert!((stars[0] - stars.last().copied().unwrap_or(0.)).abs() < 0.5, "back where it was: {stars:?}");
    }
}

/// Three favourites, the one that needs you in the middle, FAVOURITES
/// open and at rest at Full: its head, how many rows under it, and the
/// kept one's star (only a drawn row has one).
fn three_favourites(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) -> (usize, usize, String) {
    set_motion(cx, view, crate::motion::Level::Full);
    settle(cx, w, 8);
    let ids: Vec<String> = view.read_with(cx, |v, _| {
        let m = &v.model;
        let idle: Vec<String> =
            m.sessions.iter().filter(|s| s.id != crate::demo::SID && !crate::rail::busy(m, s)).take(2).map(|s| s.id.clone()).collect();
        assert!(m.sessions.iter().find(|s| s.id == crate::demo::SID).is_some_and(|s| crate::rail::busy(m, s)), "SID needs you");
        vec![idle[0].clone(), crate::demo::SID.to_string(), idle[1].clone()]
    });
    // None of them a favourite, then the three in order (the one that
    // needs you in the middle).
    let fav = |v: &AppView, id: &str| v.model.sessions.iter().find(|s| s.id == id).and_then(|s| v.model.fav_of(s)).is_some();
    for (on, id) in ids.iter().map(|id| (false, id)).chain(ids.iter().map(|id| (true, id))) {
        view.update(cx, |v, cx| {
            if fav(v, id) != on {
                v.toggle_fav_session(id);
            }
            cx.notify();
        });
        settle(cx, w, 8);
    }
    fly(cx, w, view, 400, |_| {});
    settle(cx, w, 8);
    let head = view.read_with(cx, |v, _| v.rail.rail.items.iter().position(|i| matches!(i, crate::rail::Item::Favs { open: true, .. })));
    let head = head.expect("FAVOURITES open");
    let body =
        view.read_with(cx, |v, _| v.rail.rail.items[head + 1..].iter().take_while(|i| matches!(i, crate::rail::Item::Row(_))).count());
    assert!(body >= 3, "three favourites under the head ({body})");
    // The one it keeps, as drawn (its star: only a drawn row has one).
    let key = view.read_with(cx, |v, _| crate::chrome::row_key(&v.rail.rail, &v.rail.rail.items[head + 2]).expect("a row"));
    let star = format!("star {key}");
    assert!(drawn(cx, w, star.clone()).is_some(), "its star drawn");
    (head, body, star)
}

/// The gate's S3 (PR #21): dragging the rail's edge re-wraps the column
/// at every new width (1280: under the cap), and the frame that does
/// reads the rows not yet measured at their estimates. The transcript's
/// thumb stays as it was drawn before the drag until it is let go.
#[gpui_kit::test]
fn the_thumb_holds_while_an_edge_is_dragged(cx: &mut TestAppContext) {
    let (w, view, _events) = open_at(cx, (1280., 720.), crate::theme::Panels::default().text_size);
    set_motion(cx, &view, crate::motion::Level::Full);
    view.update(cx, |v, cx| {
        v.list.scroll_to(gpui_kit::ListOffset { item_ix: 2, offset_in_item: px(13.) });
        cx.notify();
    });
    settle(cx, w, 8);
    let bar = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.bars.transcript.drawn());
    let before = bar(cx);
    assert!(before.is_some(), "the bar drawn");
    let (w0, _, _) = panel(cx, &view, true);
    let col0 = view.read_with(cx, |v, _| v.layout.column);
    press_at(cx, w, gpui_kit::point(px(w0 - 1.), px(400.)));
    for x in [w0 + 2., w0 + 5., w0 + 9., w0 + 14., w0 + 20.] {
        drag_to(cx, w, x);
        assert_eq!(bar(cx), before, "dragged to {x}: the thumb as it was");
    }
    assert!(view.read_with(cx, |v, _| v.layout.column) != col0, "the column re-wrapped");
    release_at(cx, w, gpui_kit::point(px(w0 + 20.), px(400.)));
    settle(cx, w, 8);
    fly(cx, w, &view, 400, |_| {});
    settle(cx, w, 8);
    assert!(bar(cx).is_some(), "let go: drawn from the rows again");
}

/// The gate's S6 and the landing's rewrap frame (`ff642fe`, PR #21), as
/// painted: an edge dragged out and let go near its detent springs back
/// with the transcript's thumb where it was through the whole gesture; a
/// rail fold's landing re-wraps the column, and that frame paints the
/// thumb as the one before it did (the rows are on their estimates), the
/// next from the rows.
#[gpui_kit::test]
fn the_thumb_holds_through_a_springback_and_a_landing_rewrap(cx: &mut TestAppContext) {
    let (w, view, _events) = open_at(cx, (1280., 720.), crate::theme::Panels::default().text_size);
    set_motion(cx, &view, crate::motion::Level::Full);
    view.update(cx, |v, cx| {
        v.list.scroll_to(gpui_kit::ListOffset { item_ix: 2, offset_in_item: px(13.) });
        cx.notify();
    });
    settle(cx, w, 8);
    let painted = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.bars.transcript.painted());
    let same = |a: Option<(f32, f32)>, b: (f32, f32)| a.is_some_and(|a| (a.0 - b.0).abs() < 0.5 && (a.1 - b.1).abs() < 0.5);
    let before = painted(cx).expect("the thumb painted");
    let (w0, _, _) = panel(cx, &view, true);
    press_at(cx, w, gpui_kit::point(px(w0 - 1.), px(400.)));
    for x in [w0 + 5., w0 + 12., w0 + 20.] {
        drag_to(cx, w, x);
    }
    release_at(cx, w, gpui_kit::point(px(w0 + 20.), px(400.)));
    // Every frame of the flight (landed, it paints it from the rows).
    let mut seen = Vec::new();
    fly(cx, w, &view, 400, |cx| {
        if panel(cx, &view, true).2 {
            seen.push(painted(cx));
        }
    });
    assert!(seen.len() > 5, "it sprang back ({})", seen.len());
    assert!(seen.iter().all(|&p| same(p, before)), "the thumb never moved: {before:?} {seen:?}");
    settle(cx, w, 8);
    assert!((panel(cx, &view, true).0 - w0).abs() < 0.5, "back at its detent");
    // The rail folds: the column widens when it lands (nothing else holds
    // the thumb in this flight). The frame that re-wraps paints it as the
    // one before did; the next, from the rows.
    let column = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.layout.column);
    let col0 = column(cx);
    let bars = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.bars.transcript.paints());
    bars(cx);
    catch_up(cx, &view);
    view.update(cx, |v, cx| v.toggle_panel(true, cx));
    cx.run_until_parked();
    fly(cx, w, &view, 400, |_| {});
    settle(cx, w, 8);
    let paints = bars(cx);
    assert!(column(cx) > col0 + 10., "a wider column ({col0} → {})", column(cx));
    let held: Vec<usize> = (1..paints.len()).filter(|&i| paints[i].2).collect();
    assert!(!held.is_empty() && paints.len() > 30, "the landing's frame held: {paints:?}");
    for i in held {
        let (a, b) = (paints[i - 1], paints[i]);
        assert!((a.0 - b.0).abs() < 0.5 && (a.1 - b.1).abs() < 0.5, "held as the frame before painted it: {a:?} {b:?}");
    }
    let (end, last) = (paints.last().copied().expect("painted"), paints[0]);
    assert!(!end.2 && end.0 < last.0 - 10., "then from the rows: a wider column is shorter ({last:?} {end:?})");
}

/// Shady N2 (PR #21): a panel turned round mid-flight (its fold key
/// twice) turns with its velocity (fluid.md §2.1): it goes on the way it
/// went for a frame, slowing, then back to where it was, never a step out
/// of line.
#[gpui_kit::test]
fn a_panel_turned_round_keeps_its_velocity(cx: &mut TestAppContext) {
    let (w, view, _events) = open_at(cx, (1280., 720.), crate::theme::Panels::default().text_size);
    set_motion(cx, &view, crate::motion::Level::Full);
    settle(cx, w, 8);
    for rail in [true, false] {
        let (w0, _, _) = panel(cx, &view, rail);
        catch_up(cx, &view);
        view.update(cx, |v, cx| v.toggle_panel(rail, cx));
        cx.run_until_parked();
        let mut ws = vec![w0];
        for _ in 0..8 {
            fluid_frame(cx, w, &view);
            ws.push(panel(cx, &view, rail).0);
        }
        let v0 = ws[8] - ws[7];
        assert!(v0 < -2., "{rail}: folding fast: {ws:?}");
        view.update(cx, |v, cx| v.toggle_panel(rail, cx));
        cx.run_until_parked();
        fluid_frame(cx, w, &view);
        let w1 = panel(cx, &view, rail).0;
        assert!(w1 < ws[8], "{rail}: turned round, it goes on folding for a frame first: {ws:?} {w1}");
        assert!(ws[8] - w1 <= -v0 + 0.5, "{rail}: slowing ({v0} then {})", w1 - ws[8]);
        fly(cx, w, &view, 400, |_| {});
        settle(cx, w, 8);
        assert!((panel(cx, &view, rail).0 - w0).abs() < 0.5, "{rail}: open again");
    }
}

/// Shady R2-L2 (PR #21): a section's head clicked again mid-fold (a double
/// click) turns the fold round from where it is drawn: what follows never
/// jumps, the unfold taking over the fold's offset (and the other way).
#[gpui_kit::test]
fn a_fold_turned_round_mid_way_takes_over_where_it_is(cx: &mut TestAppContext) {
    let (w, view, _events) = open_at(cx, (1280., 900.), crate::theme::Panels::default().text_size);
    set_motion(cx, &view, crate::motion::Level::Full);
    settle(cx, w, 8);
    // FAVOURITES with two favourites that go when it folds (none busy),
    // and the first item after them.
    let ids: Vec<String> = view.read_with(cx, |v, _| {
        let m = &v.model;
        m.sessions.iter().filter(|s| !crate::rail::busy(m, s)).take(2).map(|s| s.id.clone()).collect()
    });
    let fav = |v: &AppView, id: &str| v.model.sessions.iter().find(|s| s.id == id).and_then(|s| v.model.fav_of(s)).is_some();
    view.update(cx, |v, cx| {
        let all: Vec<String> = v.model.sessions.iter().map(|s| s.id.clone()).collect();
        let flip: Vec<&String> = all.iter().filter(|id| fav(v, id) != ids.contains(id)).collect();
        for id in flip {
            v.toggle_fav_session(id);
        }
        cx.notify();
    });
    settle(cx, w, 8);
    fly(cx, w, &view, 400, |_| {});
    settle(cx, w, 8);
    let next = view
        .read_with(cx, |v, _| {
            let items = &v.rail.rail.items;
            let head = items.iter().position(|i| matches!(i, crate::rail::Item::Favs { open: true, .. }))?;
            let end = (head + 1..items.len()).find(|&j| !matches!(items[j], crate::rail::Item::Row(_)))?;
            (end == head + 3).then(|| v.rail.keys[end])
        })
        .expect("FAVOURITES open with its two");
    // Where what follows is drawn: its place, and the fold's offset.
    let drawn_at = |v: &AppView| {
        let ix = v.rail.keys.iter().position(|&k| k == next)?;
        let vp = v.rail_list.viewport_bounds();
        let top = f32::from(v.rail_list.bounds_for_item(ix)?.top() - vp.top());
        Some(top + v.rail_fx.fold.as_ref().filter(|f| ix >= f.after).map_or(0., |f| f.o))
    };
    for turn in [12, 20] {
        let mut ys = vec![view.read_with(cx, |v, _| drawn_at(v)).expect("in view")];
        catch_up(cx, &view);
        view.update(cx, |v, cx| v.toggle_favs_group(cx));
        cx.run_until_parked();
        for n in 0..200 {
            if n == turn {
                view.update(cx, |v, cx| v.toggle_favs_group(cx));
                cx.run_until_parked();
            }
            fluid_frame(cx, w, &view);
            ys.extend(view.read_with(cx, |v, _| drawn_at(v)));
        }
        settle(cx, w, 8);
        let most = ys.windows(2).fold(0f32, |m, p| m.max((p[1] - p[0]).abs()));
        assert!(most < 16., "turned at frame {turn}: what follows jumped {most} px: {ys:?}");
        assert!((ys[0] - ys.last().copied().unwrap_or(0.)).abs() < 0.5, "back where it was: {ys:?}");
    }
}
