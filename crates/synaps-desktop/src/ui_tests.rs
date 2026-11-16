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
    WindowBounds, WindowOptions, px, size,
};

use crate::app::AppView;
use crate::wtabs::Act;

const DRAFT: &str = "half-typed, not for sending";

/// The window, and the events of its client (kept: its thread lives on).
fn open(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<AppView>, async_channel::Receiver<synaps_client::Event>) {
    let (window, view) = cx.update(|cx| {
        gpui_kit::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        crate::fonts::register(cx);
        let (theme, raw, _, _) = crate::theme::resolve_theme(None, None, None, None, None);
        let text = crate::theme::Panels::default().text_size;
        crate::theme::apply(crate::theme::Choice { theme, raw, accent: None, text }, cx);
        crate::app::bind_keys(cx);
        let bounds = Bounds { origin: Default::default(), size: size(px(1440.), px(900.)) };
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
