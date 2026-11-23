//! The pickers in a window: opened from the palette, on the session's own
//! value.

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, TestAppContext, VisualTestContext, WindowBounds, WindowOptions, px, size,
};
use synaps_client::{ModelOption, ModelOptions};

use crate::app::AppView;
use crate::palette::AppAct;

/// The `--offline` demo's window with a client whose daemon is nowhere (an
/// older daemon's list: the settings page's levels).
fn open(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<AppView>) {
    let (window, view) = cx.update(|cx| {
        gpui_kit::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        crate::fonts::register(cx);
        let (theme, raw, _, _) = crate::theme::resolve_theme(None, None, None, None, None);
        crate::theme::apply(crate::theme::Choice { theme, raw, accent: None, text: 0 }, cx);
        crate::app::bind_keys(cx);
        let bounds = Bounds { origin: Default::default(), size: size(px(1280.), px(800.)) };
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
    let nowhere = std::env::temp_dir().join(format!("synaps-desktop-choice-test-{}", std::process::id())).join("no-daemon.sock");
    let options = synaps_client::ClientOptions::new(synaps_client::Endpoint::Socket(nowhere), synaps_client::Identity::desktop());
    let (client, _events) = synaps_client::Client::spawn(options).expect("a client thread");
    view.update(cx, |v, cx| {
        v.client = Some(client);
        cx.notify();
    });
    cx.run_until_parked();
    (window, view)
}

/// "Change thinking…" from the palette opens on the session's level, not
/// the list's first row (finish gate F1); "Change model…" on its model.
#[gpui_kit::test]
fn a_keyboard_picker_opens_on_the_current_value(cx: &mut TestAppContext) {
    let (window, view) = open(cx);
    view.update(cx, |v, _| {
        let live = v.panes.cur.live;
        // The demo's turn over (a pick is refused while one runs).
        v.model.lives[live].transcript.apply(&synaps_client::SessionEvent::Idle);
        let s = &mut v.model.lives[live].stats;
        let mut info = s.view.clone().unwrap_or_default();
        info.effort = "low".into();
        s.view = Some(info);
    });
    let sel = |cx: &mut TestAppContext| {
        view.read_with(cx, |v, _| {
            let (m, t) = v.choice.menu.as_ref().expect("a picker is open");
            assert!(t.off.is_none(), "the demo session takes a pick: {:?}", t.off);
            m.sel.and_then(|i| m.items.get(i)).map(|it| it.label.to_string())
        })
    };
    cx.update_window(window, |_, window, cx| view.update(cx, |v, cx| v.app_act(AppAct::ChangeThinking, window, cx))).unwrap();
    cx.run_until_parked();
    assert_eq!(sel(cx).as_deref(), Some("low"), "on the current level, not `off`");
    view.update(cx, |v, cx| {
        v.close_menu();
        cx.notify();
    });
    cx.run_until_parked();
    let model = view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].stats.view.as_ref().map(|i| i.model.clone()).unwrap_or_default());
    cx.update_window(window, |_, window, cx| view.update(cx, |v, cx| v.app_act(AppAct::ChangeModel, window, cx))).unwrap();
    cx.run_until_parked();
    if !model.is_empty() {
        assert_eq!(sel(cx).as_deref(), Some(model.as_str()), "on the current model");
    }
}

/// The daemon's list arriving after a keyboard open keeps the selection on
/// the current model, not on whatever row now has its index, so Enter
/// changes nothing; a selection the keys moved stays on its model through
/// a reordered list (finish gate R1, Shady #72).
#[gpui_kit::test]
fn the_daemon_s_late_list_keeps_a_keyboard_picker_on_its_model(cx: &mut TestAppContext) {
    let (window, view) = open(cx);
    let m = |id: &str, label: &str, provider: &str| ModelOption {
        id: id.into(),
        label: label.into(),
        provider: provider.into(),
        provider_key: provider.to_lowercase(),
    };
    let (sonnet, opus, gpt) = (
        m("anthropic/claude-sonnet-4-6", "Sonnet 4.6", "Anthropic"),
        m("anthropic/claude-opus-5-5", "Opus 5.5", "Anthropic"),
        m("openai-codex/gpt-5.5", "GPT-5.5", "OpenAI Codex"),
    );
    let list = |models: Vec<ModelOption>| ModelOptions { model: "anthropic/claude-opus-5-5".into(), models, ..ModelOptions::default() };
    let seq = view.update(cx, |v, _| {
        let live = v.panes.cur.live;
        v.model.lives[live].transcript.apply(&synaps_client::SessionEvent::Idle);
        let s = &mut v.model.lives[live].stats;
        let mut info = s.view.clone().unwrap_or_default();
        info.model = "anthropic/claude-opus-5-5".into();
        s.view = Some(info);
        v.model.lives[live].attachment().expect("attached")
    });
    // From the palette, as a supported daemon with nothing cached opens it:
    // the current model alone while the list is asked for.
    let open_picker = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| view.update(cx, |v, cx| v.app_act(AppAct::ChangeModel, window, cx))).unwrap();
        view.update(cx, |v, _| {
            let (_, t) = v.choice.menu.as_mut().expect("a picker is open");
            (t.note, t.loading) = (None, true);
        });
        cx.run_until_parked();
    };
    let reply = |cx: &mut TestAppContext, models: Vec<ModelOption>| {
        view.update(cx, |v, cx| {
            v.choice_options(seq, Ok(list(models)));
            cx.notify();
        });
        cx.run_until_parked();
    };
    let key = |cx: &mut TestAppContext, k: &'static str| {
        let ks = gpui_kit::Keystroke::parse(k).unwrap();
        cx.update_window(window, |_, window, cx| view.update(cx, |v, cx| v.choice_keydown(&ks, true, window, cx))).unwrap();
        cx.run_until_parked();
    };
    let sel = |cx: &mut TestAppContext| {
        view.read_with(cx, |v, _| {
            let (m, t) = v.choice.menu.as_ref().expect("a picker is open");
            assert!(t.off.is_none(), "the demo session takes a pick: {:?}", t.off);
            m.sel.and_then(|i| m.items.get(i)).map(|it| it.label.to_string())
        })
    };

    open_picker(cx);
    assert_eq!(sel(cx).as_deref(), Some("anthropic/claude-opus-5-5"), "the current model, before the list");
    reply(cx, vec![sonnet.clone(), opus.clone(), gpt.clone()]);
    assert_eq!(sel(cx).as_deref(), Some("Opus 5.5"), "still the current model, second in the list");
    key(cx, "enter");
    let after = view.read_with(cx, |v, _| v.choice.menu.as_ref().map(|(_, t)| t.pending));
    assert_eq!(after, None, "Enter on the current model closes the picker and sets nothing");

    open_picker(cx);
    reply(cx, vec![sonnet.clone(), opus.clone(), gpt.clone()]);
    key(cx, "up");
    assert_eq!(sel(cx).as_deref(), Some("Sonnet 4.6"), "moved by the keys");
    reply(cx, vec![gpt, opus, sonnet]);
    assert_eq!(sel(cx).as_deref(), Some("Sonnet 4.6"), "kept on its model through a reordered list");
}

/// Frames until the window asks for none (at most 12).
fn settle(cx: &mut TestAppContext, w: AnyWindowHandle) {
    for _ in 0..12 {
        let ran = cx.update_window(w, |_, window, cx| window.simulate_next_frame(cx)).expect("the window");
        cx.run_until_parked();
        if ran == 0 {
            return;
        }
    }
}

/// The open picker's width as drawn.
fn menu_width(cx: &mut TestAppContext, w: AnyWindowHandle) -> f32 {
    let b = VisualTestContext::from_window(w, cx).debug_bounds("choice-menu-box").expect("the picker is drawn");
    f32::from(b.size.width)
}

/// Another provider's (narrower) rows do not narrow the open menu under
/// the pointer, and back keeps it too (finish gate F2).
#[gpui_kit::test]
fn a_provider_swap_keeps_the_menu_s_width(cx: &mut TestAppContext) {
    let (window, view) = open(cx);
    let m = |id: &str, label: &str, provider: &str| ModelOption {
        id: id.into(),
        label: label.into(),
        provider: provider.into(),
        provider_key: provider.to_lowercase(),
    };
    let opts = ModelOptions {
        model: "wide/a".into(),
        models: vec![
            m("wide/a", "A model whose label is long — the widest row here", "Wide"),
            m("wide/b", "B", "Wide"),
            m("narrow/c", "C", "Narrow"),
        ],
        ..ModelOptions::default()
    };
    let seq = view.update(cx, |v, _| {
        let live = v.panes.cur.live;
        v.model.lives[live].transcript.apply(&synaps_client::SessionEvent::Idle);
        let s = &mut v.model.lives[live].stats;
        let mut info = s.view.clone().unwrap_or_default();
        info.model = "wide/a".into();
        s.view = Some(info);
        v.model.lives[live].attachment().expect("attached")
    });
    cx.update_window(window, |_, window, cx| view.update(cx, |v, cx| v.app_act(AppAct::ChangeModel, window, cx))).unwrap();
    view.update(cx, |v, cx| {
        // The nowhere daemon's "does not list models" note goes: the rows'
        // own widths decide the box.
        if let Some((_, t)) = v.choice.menu.as_mut() {
            t.note = None;
        }
        v.choice_options(seq, Ok(opts));
        cx.notify();
    });
    settle(cx, window);
    let wide = menu_width(cx, window);
    let provider = |cx: &mut TestAppContext| {
        view.read_with(cx, |v, _| {
            let (m, _) = v.choice.menu.as_ref().expect("open");
            m.items.iter().find(|it| it.act >= super::PROVIDER && it.act != super::NOTE).map(|it| it.act).expect("a provider row")
        })
    };
    let row = provider(cx);
    cx.update_window(window, |_, window, cx| view.update(cx, |v, cx| v.choice_pick(row, crate::menu::How::Click(1), window, cx))).unwrap();
    settle(cx, window);
    let labels =
        view.read_with(cx, |v, _| v.choice.menu.as_ref().map(|(m, _)| m.items.iter().map(|i| i.label.to_string()).collect::<Vec<_>>()));
    assert!(labels.as_ref().is_some_and(|l| l.iter().any(|s| s == "C")), "the narrow provider's models: {labels:?}");
    assert_eq!(menu_width(cx, window), wide, "the box keeps its width");
    let row = provider(cx);
    cx.update_window(window, |_, window, cx| view.update(cx, |v, cx| v.choice_pick(row, crate::menu::How::Click(1), window, cx))).unwrap();
    settle(cx, window);
    assert_eq!(menu_width(cx, window), wide, "and back");
}
