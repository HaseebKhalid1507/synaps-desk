//! The pickers in a window: opened from the palette, on the session's own
//! value.

use crate::model_list::{List, Listed};
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, TestAppContext, VisualTestContext, WindowBounds, WindowOptions, px, size,
};

use crate::app::AppView;
use crate::palette::AppAct;

/// The `--offline` demo's window with a client whose daemon is nowhere
/// (no `Welcome`: no list is read; a test gives it one).
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

/// A list arriving after a keyboard open keeps the selection on
/// the current model, not on whatever row now has its index, so Enter
/// changes nothing; a selection the keys moved stays on its model through
/// a reordered list (finish gate R1, Shady #72).
#[gpui_kit::test]
fn the_daemon_s_late_list_keeps_a_keyboard_picker_on_its_model(cx: &mut TestAppContext) {
    let (window, view) = open(cx);
    let m = |id: &str, label: &str, provider: &str| Listed {
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
    let list = |models: Vec<Listed>| List { models, ..List::default() };
    view.update(cx, |v, _| {
        let live = v.panes.cur.live;
        v.model.lives[live].transcript.apply(&synaps_client::SessionEvent::Idle);
        let s = &mut v.model.lives[live].stats;
        let mut info = s.view.clone().unwrap_or_default();
        info.model = "anthropic/claude-opus-5-5".into();
        s.view = Some(info);
        assert!(v.model.lives[live].attachment().is_some(), "attached");
    });
    // From the palette, as a daemon with nothing cached opens it: the
    // current model alone while the list is read.
    let open_picker = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| view.update(cx, |v, cx| v.app_act(AppAct::ChangeModel, window, cx))).unwrap();
        view.update(cx, |v, _| {
            let (_, t) = v.choice.menu.as_mut().expect("a picker is open");
            (t.note, t.loading) = (None, true);
        });
        cx.run_until_parked();
    };
    let reply = |cx: &mut TestAppContext, models: Vec<Listed>| {
        view.update(cx, |v, cx| {
            v.choice_listed(None, list(models));
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

/// A late list keeps the provider the keys are in and the model selected
/// there, by its full id: Google AI Studio's Flash is not Google Gemini's
/// (the same name; another route and credential). A selected model the
/// list no longer has leaves nothing selected, so Enter sends nothing; a
/// pick sends the browsed provider's own id (Shady #72 round 2).
#[gpui_kit::test]
fn a_late_list_keeps_the_browsed_provider_s_model(cx: &mut TestAppContext) {
    let (window, view) = open(cx);
    let m = |id: &str, label: &str, key: &str, provider: &str| Listed {
        id: id.into(),
        label: label.into(),
        provider: provider.into(),
        provider_key: key.into(),
    };
    let current = "google-gemini/gemini-2.5-pro";
    let models = vec![
        m(current, "Gemini 2.5 Pro", "google-gemini", "Google Gemini"),
        m("google-gemini/gemini-2.5-flash", "Gemini 2.5 Flash", "google-gemini", "Google Gemini"),
        m("google/gemini-2.5-flash", "Gemini 2.5 Flash", "google", "Google AI Studio"),
        m("google/gemini-2.5-pro", "Gemini 2.5 Pro", "google", "Google AI Studio"),
    ];
    view.update(cx, |v, _| {
        let live = v.panes.cur.live;
        v.model.lives[live].transcript.apply(&synaps_client::SessionEvent::Idle);
        let s = &mut v.model.lives[live].stats;
        let mut info = s.view.clone().unwrap_or_default();
        info.model = current.into();
        s.view = Some(info);
        assert!(v.model.lives[live].attachment().is_some(), "attached");
    });
    let reply = |cx: &mut TestAppContext, models: Vec<Listed>| {
        view.update(cx, |v, cx| {
            v.choice_listed(None, List { models, ..List::default() });
            cx.notify();
        });
        cx.run_until_parked();
    };
    let key = |cx: &mut TestAppContext, k: &'static str| {
        let ks = gpui_kit::Keystroke::parse(k).unwrap();
        cx.update_window(window, |_, window, cx| view.update(cx, |v, cx| v.choice_keydown(&ks, true, window, cx))).unwrap();
        cx.run_until_parked();
    };
    // The selected row: its label, and its model's id if it is one.
    let sel = |cx: &mut TestAppContext| {
        view.read_with(cx, |v, _| {
            let (m, t) = v.choice.menu.as_ref().expect("a picker is open");
            assert!(t.off.is_none(), "the demo session takes a pick: {:?}", t.off);
            let it = m.sel.and_then(|i| m.items.get(i));
            (it.map(|it| it.label.to_string()), it.and_then(|it| t.values.get(it.act)).map(|v| v.0.clone()))
        })
    };
    // What a pick sent `Set` for.
    let sent = |cx: &mut TestAppContext| {
        view.read_with(cx, |v, _| v.choice.menu.as_ref().and_then(|(_, t)| t.pending.and_then(|i| t.values.get(i)).map(|v| v.0.clone())))
    };

    cx.update_window(window, |_, window, cx| view.update(cx, |v, cx| v.app_act(AppAct::ChangeModel, window, cx))).unwrap();
    view.update(cx, |v, _| {
        let (_, t) = v.choice.menu.as_mut().expect("a picker is open");
        (t.note, t.loading) = (None, true);
    });
    cx.run_until_parked();
    reply(cx, models.clone());
    assert_eq!(sel(cx).1.as_deref(), Some(current));
    key(cx, "down");
    key(cx, "down");
    assert_eq!(sel(cx).0.as_deref(), Some("Google AI Studio · 2"));
    key(cx, "enter");
    assert_eq!(sel(cx).1.as_deref(), Some("google/gemini-2.5-flash"), "Google AI Studio's models, its first");
    reply(cx, models.clone());
    assert_eq!(sel(cx).1.as_deref(), Some("google/gemini-2.5-flash"), "the same list: still Google AI Studio's Flash");
    reply(cx, models.iter().filter(|m| m.id != "google/gemini-2.5-flash").cloned().collect());
    assert_eq!(sel(cx), (None, None), "gone: nothing selected, not Google Gemini's Flash");
    key(cx, "enter");
    assert_eq!(sent(cx), None, "Enter on nothing sends nothing");
    reply(cx, models);
    assert_eq!(sel(cx), (None, None), "nothing stays nothing");
    key(cx, "down");
    assert_eq!(sel(cx).1.as_deref(), Some("google/gemini-2.5-flash"), "Google AI Studio's still");
    key(cx, "enter");
    assert_eq!(sent(cx).as_deref(), Some("google/gemini-2.5-flash"), "its own id");
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
    let m = |id: &str, label: &str, provider: &str| Listed {
        id: id.into(),
        label: label.into(),
        provider: provider.into(),
        provider_key: provider.to_lowercase(),
    };
    let opts = List {
        models: vec![
            m("wide/a", "A model whose label is long — the widest row here", "Wide"),
            m("wide/b", "B", "Wide"),
            m("narrow/c", "C", "Narrow"),
        ],
        ..List::default()
    };
    view.update(cx, |v, _| {
        let live = v.panes.cur.live;
        v.model.lives[live].transcript.apply(&synaps_client::SessionEvent::Idle);
        let s = &mut v.model.lives[live].stats;
        let mut info = s.view.clone().unwrap_or_default();
        info.model = "wide/a".into();
        s.view = Some(info);
        assert!(v.model.lives[live].attachment().is_some(), "attached");
    });
    cx.update_window(window, |_, window, cx| view.update(cx, |v, cx| v.app_act(AppAct::ChangeModel, window, cx))).unwrap();
    view.update(cx, |v, cx| {
        // The nowhere daemon's "not connected" note goes: the rows'
        // own widths decide the box.
        if let Some((_, t)) = v.choice.menu.as_mut() {
            t.note = None;
        }
        v.choice_listed(None, opts);
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

/// The daemon decides (no list says what a session takes): a model pick it
/// applies with a clamp closes the menu, says the clamp on the composer's
/// note, and the thinking label is the level its `setting_changed` view
/// says; a refused pick keeps the old value and says why.
#[gpui_kit::test]
fn a_clamp_shows_the_daemon_s_level_and_a_refusal_keeps_the_old_one(cx: &mut TestAppContext) {
    let (window, view) = open(cx);
    let seq = view.update(cx, |v, _| {
        let live = v.panes.cur.live;
        v.model.lives[live].transcript.apply(&synaps_client::SessionEvent::Idle);
        let s = &mut v.model.lives[live].stats;
        let mut info = s.view.clone().unwrap_or_default();
        (info.model, info.effort) = ("anthropic/claude-opus-5-5".into(), "max".into());
        s.view = Some(info);
        v.model.lives[live].attachment().expect("attached")
    });
    let shown = |cx: &mut TestAppContext| {
        view.read_with(cx, |v, _| {
            let info = v.model.lives[v.panes.cur.live].stats.view.clone().unwrap_or_default();
            (info.model, info.effort, v.panes.cur.watch_note.as_ref().map(|n| n.to_string()))
        })
    };
    let pick = |cx: &mut TestAppContext, value: &str| {
        let i = view.read_with(cx, |v, _| {
            let (_, t) = v.choice.menu.as_ref().expect("a picker is open");
            t.values.iter().position(|(id, _)| id == value).expect("listed")
        });
        cx.update_window(window, |_, window, cx| view.update(cx, |v, cx| v.choice_pick(i, crate::menu::How::Click(1), window, cx)))
            .unwrap();
        cx.run_until_parked();
        let pending = view.read_with(cx, |v, _| v.choice.menu.as_ref().and_then(|(_, t)| t.pending));
        assert_eq!(pending, Some(i), "asked, the label unchanged until the daemon says");
    };

    cx.update_window(window, |_, window, cx| view.update(cx, |v, cx| v.app_act(AppAct::ChangeModel, window, cx))).unwrap();
    view.update(cx, |v, cx| {
        v.choice_listed(None, List::of(&["anthropic/claude-opus-5-5", "anthropic/claude-haiku-4-5"]));
        cx.notify();
    });
    cx.run_until_parked();
    pick(cx, "anthropic/claude-haiku-4-5");
    let clamp = "thinking → high (clamped from max: not supported by anthropic/claude-haiku-4-5)";
    view.update(cx, |v, cx| {
        let raw = serde_json::json!({"ev": "setting_changed", "applied": {"id": 7, "setting": "model", "ok": true, "message": clamp,
            "view": {"model": "anthropic/claude-haiku-4-5", "thinking_level": "high"}, "clamp": {"from": "max", "to": "high"}}});
        let ev = synaps_client::protocol::decode_event(&raw).expect("decodes");
        let live = v.panes.cur.live;
        v.model.lives[live].stats.on_session(&ev, &raw);
        v.choice_answered(seq, "model", Ok(Some(clamp.into())));
        cx.notify();
    });
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| v.choice.menu.is_none()), "applied: the menu closes");
    assert_eq!(shown(cx), ("anthropic/claude-haiku-4-5".into(), "high".into(), Some(clamp.into())), "the daemon's level, and why");

    cx.update_window(window, |_, window, cx| view.update(cx, |v, cx| v.app_act(AppAct::ChangeThinking, window, cx))).unwrap();
    cx.run_until_parked();
    pick(cx, "max");
    view.update(cx, |v, cx| {
        v.choice_answered(seq, "reasoning_level", Err("max is not supported by anthropic/claude-haiku-4-5".into()));
        cx.notify();
    });
    cx.run_until_parked();
    let refused = view.read_with(cx, |v, _| v.choice.menu.as_ref().and_then(|(_, t)| t.refused.clone()).map(|r| r.1));
    assert_eq!(refused.as_deref(), Some("max is not supported by anthropic/claude-haiku-4-5"), "said in the open menu");
    let (_, level, note) = shown(cx);
    assert_eq!(level, "high", "the old level stands");
    assert_eq!(note.as_deref(), Some("thinking not changed: max is not supported by anthropic/claude-haiku-4-5"));
}

/// The hard rule in a window: a config with `provider.*` keys and other
/// secrets, read for the model picker from a file and through a command
/// that prints all of it: the favourites are listed, and nothing the
/// window holds or says (the menu's rows and their tooltips, its target,
/// the composer's note) nor the log has any of the rest.
#[gpui_kit::test]
fn a_config_s_secrets_never_reach_the_picker(cx: &mut TestAppContext) {
    use crate::model_list::{Source, Where};
    let logged = crate::model_list::tests::capture_logs();
    let (window, view) = open(cx);
    let dir = std::env::temp_dir().join(format!("synaps-desktop-choice-secrets-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let config = dir.join("config");
    std::fs::write(
        &config,
        "provider.groq = gsk_SECRET_ONE\nprovider.openai = sk-SECRET-TWO\nauth.token = SECRET_THREE\nSECRET_FOUR\n\
         favorite_models = anthropic/claude-fable-5-2, groq/llama-4\nfavorite_models_old = SECRET_SIX\n",
    )
    .unwrap();
    view.update(cx, |v, _| {
        let live = v.panes.cur.live;
        v.model.lives[live].transcript.apply(&synaps_client::SessionEvent::Idle);
        let s = &mut v.model.lives[live].stats;
        let mut info = s.view.clone().unwrap_or_default();
        info.model = "anthropic/claude-opus-5-5".into();
        s.view = Some(info);
        // A daemon (pid 0: never this machine's), so the list is read.
        v.model.daemon = Some(crate::model::Daemon { version: "0".into(), profile: None, pid: 0 });
    });
    let cat = format!("cat '{}'", config.display());
    for favorites in [Where::File(config.clone()), Where::Command(cat.clone()), Where::Command(format!("{cat}; exit 3"))] {
        let failing = matches!(&favorites, Where::Command(c) if c.ends_with("exit 3"));
        view.update(cx, |v, cx| {
            v.close_menu();
            v.choice = crate::choice::Choices::default();
            v.app_settings.pickers = Source { favorites, logins: Where::File(config.clone()), warning: None };
            cx.notify();
        });
        cx.update_window(window, |_, window, cx| view.update(cx, |v, cx| v.app_act(AppAct::ChangeModel, window, cx))).unwrap();
        cx.run_until_parked();
        settle(cx, window);
        let (said, values) = view.read_with(cx, |v, _| {
            let (m, t) = v.choice.menu.as_ref().expect("a picker is open");
            (format!("{m:?} {t:?} {:?} {:?}", v.panes.cur.watch_note, v.panes.cur.quiet_note), t.values.clone())
        });
        crate::model_list::tests::assert_no_secret("the picker", &said);
        let ids: Vec<&str> = values.iter().map(|v| v.0.as_str()).collect();
        if failing {
            assert_eq!(ids, ["anthropic/claude-opus-5-5"], "the session's model only: {said}");
            assert!(said.contains("favourites not read") && said.contains("failed (exit 3)"), "{said}");
        } else {
            assert_eq!(ids, ["anthropic/claude-opus-5-5", "anthropic/claude-fable-5-2"], "{said}");
            assert!(said.contains("groq · 1") && said.contains("logins not checked"), "{said}");
        }
    }
    crate::model_list::tests::assert_no_secret("the log", &logged.lock().unwrap());
}
