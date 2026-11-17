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
    assert_eq!(motion_pending(cx, &view), (false, None), "settled: no frame, no timer");
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

/// The answered card fades in its own slot (the dock keeps its height),
/// drawn as it last was and taking no click; then the dock closes in one
/// frame. Off: it closes at once, as today.
#[gpui_kit::test]
fn the_answered_card_fades_in_its_slot(cx: &mut TestAppContext) {
    use std::time::Duration;
    let (w, view, _events) = open(cx);
    set_motion(cx, &view, crate::motion::Level::Full);
    settle(cx, w, 8);
    let bounds = |cx: &mut TestAppContext, sel: &'static str| VisualTestContext::from_window(w, cx).debug_bounds(sel);
    // A new prompt arrives while watched (the one there at open is history:
    // it doesn't breathe): its card traces and its glyph breathes.
    resolve_prompt(cx, &view);
    view.update(cx, |v, cx| {
        let request = synaps_client::protocol::PromptRequest {
            id: 8,
            kind: serde_json::json!("confirm"),
            title: "Confirm tool call".into(),
            prompt: "Tool: bash\n\nAllow this tool call?".into(),
        };
        v.model.transcript.apply(&SessionEvent::Prompt { request });
        cx.notify();
    });
    cx.run_until_parked();
    settle(cx, w, 8);
    let live = bounds(cx, "card").expect("the new card is up");
    let in_card = |cx: &mut TestAppContext, b: gpui_kit::Bounds<gpui_kit::Pixels>| {
        view.read_with(cx, |v, _| {
            v.fx.sites().iter().filter(|s| matches!(s.beat, crate::motion::Beat::Breath(_)) && b.contains(&s.origin)).count()
        })
    };
    assert_eq!(in_card(cx, live), 1, "the live card's glyph breathes on the beat layer");
    // The live card takes a press (the control for the leaving one below).
    click_at(cx, w, live.center());
    assert_eq!(view.read_with(cx, |v, _| v.card_presses), 1, "the live card took the press");
    resolve_prompt(cx, &view);
    // Held mid-fade for the test.
    view.update(cx, |v, cx| {
        v.card_exit.as_mut().expect("answered on screen: it fades").at = v.fx.clock() + Duration::from_secs(60);
        cx.notify();
    });
    cx.run_until_parked();
    assert!(bounds(cx, "card").is_none());
    let ghost = bounds(cx, "card-exit").expect("the answered card, fading");
    assert_eq!(ghost, live, "in its slot: nothing above or below it moved");
    assert_eq!(in_card(cx, ghost), 0, "leaving, its glyph holds still: no breath over its cover");
    click_at(cx, w, ghost.center());
    assert_eq!(view.read_with(cx, |v, _| v.card_presses), 1, "the leaving card took no press (the one is the live card's)");
    // Its time is up: the dock closes in one frame.
    view.update(cx, |v, cx| {
        v.card_exit.as_mut().unwrap().at = v.fx.clock() - Duration::from_secs(1);
        cx.notify();
    });
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| v.card_exit.is_none()));
    assert!(bounds(cx, "card-exit").is_none() && bounds(cx, "card").is_none());
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
    assert_eq!(motion_pending(cx, &view), (false, None), "Idle CPU runs nothing: no frame, no timer");
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

/// Soul B (motion.md §10 amendment 15): a running call's lit bar blooms,
/// a 1 px column beside it in the row's padding (nothing moves: the
/// bloom is an absolute child); Off has none; it is gone once the bar
/// has settled after the call ends.
#[gpui_kit::test]
fn a_running_bar_blooms_until_it_settles(cx: &mut TestAppContext) {
    use synaps_client::protocol::{LlmEvent, StreamEvent};
    let (w, view, _events) = open_at(cx, (1440., 1800.), crate::theme::Panels::default().text_size);
    set_motion(cx, &view, crate::motion::Level::Full);
    // Read off a frame of the app's own (a beat may have replayed it).
    let blooms = |cx: &mut TestAppContext| {
        view.update(cx, |_, cx| cx.notify());
        cx.update_window(w, |_, window, cx| window.simulate_next_frame(cx)).expect("the window");
        cx.run_until_parked();
        VisualTestContext::from_window(w, cx).debug_bounds("bar-bloom")
    };
    // The demo's own calls have ended: nothing blooms.
    for _ in 0..6 {
        beat(cx, w, &view);
    }
    assert!(blooms(cx).is_none(), "no call runs: no bloom");
    running_tools(cx, &view, 1);
    for _ in 0..4 {
        beat(cx, w, &view);
    }
    let b = blooms(cx).expect("the running call's bar blooms");
    assert_eq!(b.size.width, gpui_kit::px(1.), "1 px");
    set_motion(cx, &view, crate::motion::Level::Off);
    settle(cx, w, 4);
    assert!(blooms(cx).is_none(), "Off: the static app, no bloom");
    set_motion(cx, &view, crate::motion::Level::Full);
    settle(cx, w, 4);
    assert!(blooms(cx).is_some(), "lit again");
    view.update(cx, |v, cx| {
        v.model.transcript.apply(&SessionEvent::Stream {
            event: StreamEvent::Llm(LlmEvent::ToolResult { tool_id: "ui-run-0".into(), result: "ok".into() }),
        });
        v.sync_list();
        cx.notify();
    });
    cx.run_until_parked();
    // (Its light decays over SLOW: not sampled mid-way.)
    for _ in 0..5 {
        beat(cx, w, &view);
    }
    assert!(blooms(cx).is_none(), "settled to its hue: the bloom is gone");
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
    let scans = sites.iter().filter(|s| matches!(s.beat, crate::motion::Beat::Scan(_))).count();
    assert!(scans >= 25, "the running glyphs on screen are the layer's: {scans}");
    let (f0, l0) = frames(cx, &view);
    let p0 = view.read_with(cx, |v, _| v.fx.painted.borrow().len());
    let mut layer_only = 0;
    for _ in 0..16 {
        layer_only += usize::from(view.read_with(cx, |v, _| v.fx.wakes()) == Some(crate::motion::Wake::Beat));
        beat(cx, w, &view);
    }
    let (f1, l1) = frames(cx, &view);
    // 2 s: the clocks' second twice (a third at most, as the edges fall).
    assert!(layer_only >= 13, "the timer woke only the layer on {layer_only} of 16 beats");
    // Every beat paints it, once (Shady N2: a beat painted twice is the
    // regression the layer exists to prevent): a beat is painted again
    // only by an app frame inside it.
    assert_painted_once(cx, &view, p0, l1 - l0, f1 - f0);
    assert!(f1 - f0 <= 3, "16 beats drew the app {} times: only the clocks' second may", f1 - f0);
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
}

/// Finish gate fix 1: a turn that ends here hands the floor back: Handoff
/// in's light travels down the composer's bar and is gone after `sig`
/// (its colour, decaying into the bar: `motion::handoff_in_decays_into_the_bar`);
/// Off draws none.
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
    assert!(seg(cx).is_none());
    busy(cx, false);
    assert_eq!(view.read_with(cx, |v, _| v.model.tabs.current().map(|t| v.tab_state(t))), Some(crate::rail::State::Idle), "READY");
    assert!(seg(cx).is_some(), "the turn ended here: the light travels down the bar");
    for _ in 0..6 {
        beat(cx, w, &view);
    }
    assert!(seg(cx).is_none(), "gone after sig");
    set_motion(cx, &view, crate::motion::Level::Off);
    busy(cx, true);
    busy(cx, false);
    assert!(seg(cx).is_none(), "Off: none");
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
    let scans = view.read_with(cx, |v, _| v.fx.sites().iter().filter(|s| matches!(s.beat, crate::motion::Beat::Scan(_))).count());
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
    assert!(painted.len() >= 12, "about a paint a beat: {painted:?}");
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
/// frame, and the running glyph's beam is drawn in it, one row down a beat.
#[gpui_kit::test]
fn the_beat_in_the_frame_scans_without_the_layer(cx: &mut TestAppContext) {
    let (w, view, _events) = open_with(cx, (1440., 900.), crate::theme::Panels::default().text_size, false);
    running_without_a_clock_in_frame(cx, w, &view);
    let beam = |cx: &mut TestAppContext| VisualTestContext::from_window(w, cx).debug_bounds("beam-row").expect("a beam row in the frame");
    let at = |cx: &mut TestAppContext| view.read_with(cx, |v, _| v.fx.frame_beat());
    let (f0, b0, k0) = (frames(cx, &view).0, beam(cx), at(cx));
    assert_eq!(view.read_with(cx, |v, _| v.fx.wakes()), Some(crate::motion::Wake::App), "the timer wakes the app");
    beat(cx, w, &view);
    let (f1, b1, k1) = (frames(cx, &view).0, beam(cx), at(cx));
    assert!(f1 > f0 && k1 > k0, "the beat is the app's frame");
    assert!(view.read_with(cx, |v, _| v.fx.sites().is_empty()), "nothing on the layer");
    // The row of the frame's own beat (one down a beat, back to the top
    // after 8; a slow runner's clock may have crossed two).
    let dy = f32::from(b1.origin.y - b0.origin.y);
    assert_eq!(dy, crate::glyph::scan_row(k1) - crate::glyph::scan_row(k0), "the beam on the frame's beat ({k0} → {k1})");
    // Off: still, no beam.
    set_motion(cx, &view, crate::motion::Level::Off);
    assert!(VisualTestContext::from_window(w, cx).debug_bounds("beam-row").is_none());
    assert_eq!(motion_pending(cx, &view), (false, None));
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
    let sites = view.read_with(cx, |v, _| v.fx.sites().len());
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
    assert_eq!(view.read_with(cx, |v, _| v.fx.sites().len()), sites + 1, "the new call's glyph is on the layer");
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
    // The key: the session on screen.
    let shown = view.read_with(cx, |v, _| v.model.tabs.current().and_then(|t| t.session.clone()).expect("the demo's session"));
    down(cx, w, if cfg!(target_os = "macos") { "cmd-d" } else { "ctrl-d" }, false);
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
