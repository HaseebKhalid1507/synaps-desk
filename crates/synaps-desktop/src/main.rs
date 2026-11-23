//! Synaps Desktop (GPUI spike): a thin client for the Synaps daemon.
//!
//! ```text
//! synaps-desktop [--profile P | --socket PATH | --offline] [--session ID] [--theme NAME] [--accent #RRGGBB] [--accent-picker] [--palette goto|commands] [--size WxH] [--dash URL|FILE] [--motion full|reduced|off]
//! synaps-desktop --bench [--profile P --prompt TEXT] [--refresh-hz HZ]
//! synaps-desktop --bench-tabs [--bench-history FILE] [--refresh-hz HZ]
//! synaps-desktop --bench-motion [--refresh-hz HZ]
//! synaps-desktop --agent-surface FILE [--theme NAME] [--accent #RRGGBB] [--size WxH] [--motion full|reduced|off]
//! ```
//! `--bench` connects to a daemon only when `--profile` or `--socket` is
//! given (never the default daemon), runs the synthetic scenarios, prints
//! JSON and exits (watchdog: 30 s). `--offline` shows the synthetic
//! transport's sessions ([`demo`]). The theme (one of the Synaps TUI's):
//! `--theme`, else `$SYNAPS_THEME`, else the settings file the in-app picker
//! writes ([`theme::settings_path`]), else `theme =` in the Synaps config,
//! else nord. The accent override: `--accent`, else
//! `$SYNAPS_DESKTOP_ACCENT`, else the settings file, else none. The same
//! file keeps which side panels are folded (Ctrl+B, Ctrl+Alt+B), the
//! text size (Ctrl+= / Ctrl+- / Ctrl+0) and the open tabs (per daemon);
//! `--bench` reads none of them.
//! `--size`: the window's size in logical pixels (screenshots).
//! Motion ([`motion`]): `$SYNAPS_NO_BOOT_FX` set turns it off, else
//! `--motion` for this run, else the settings file, else the OS's.

// serde_json's `json!` in the synthetic transport and its tests nests deep.
#![recursion_limit = "256"]
// The view-level split tests (`split_tests`) need a unix socket, so they only
// build on unix. Test-only helpers whose one caller is there are dead code in
// a Windows test build, and `-D warnings` failed CI on windows-latest for
// each new one (#32 `enter`, #36 `parked`, #47 `thumbs`). Linux clippy, where
// every test module builds, stays the authority on dead code.
#![cfg_attr(all(test, not(unix)), allow(dead_code))]

mod agent_surface;
mod app;
mod authorize;
mod beat;
mod bench;
mod cards;
mod chart;
mod choice;
mod chrome;
mod dash;
mod demo;
mod diag;
mod dnd;
mod events;
mod favourites;
mod files;
mod fit;
mod fonts;
mod glyph;
mod lattice;
mod live;
mod mem;
mod menu;
mod model;
mod motion;
mod named;
mod overlay;
mod palette;
mod palettes;
mod pane;
mod rail;
#[cfg(test)]
mod receipt_tests;
mod scroll;
mod scrollbar;
mod settings;
mod settings_view;
mod shell;
mod split;
#[cfg(all(test, unix))]
mod split_tests;
mod stats;
mod sticky;
#[cfg(test)]
mod style_tests;
mod subagents;
mod tabs;
mod tabstrip;
#[cfg(test)]
mod test_kit;
#[cfg(all(test, unix))]
mod test_waits;
mod theme;
mod times;
mod tray;
mod turns;
#[cfg(test)]
mod ui_tests;
mod usage;
mod usage_view;
mod views;
mod watch;
mod wtabs;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::*;
use synaps_client::{Client, ClientOptions, Endpoint, Identity};

fn main() {
    let t_main = Instant::now();
    diag::init(t_main);
    let exec_to_main_ms = mem::process_age_ms();
    let mut profile: Option<String> = None;
    let mut socket: Option<PathBuf> = None;
    let mut offline = false;
    let mut surface_file = None;
    let mut bench = false;
    let mut diag_mode = false;
    let mut bench_tabs = false;
    let mut bench_motion = false;
    let mut bench_panels = false;
    let mut history_file: Option<PathBuf> = None;
    let mut prompt: Option<String> = None;
    let mut refresh_hz: Option<f64> = None;
    let mut accent: Option<String> = None;
    let mut theme_flag: Option<String> = None;
    let mut start_session: Option<String> = None;
    let mut open_picker = false;
    let mut open_palette = None;
    let mut window_size: Option<(f32, f32)> = None;
    let mut dash_flag: Option<String> = None;
    let mut motion_flag: Option<String> = None;
    let mut args = std::env::args().skip(1);
    // A flag's value; another flag in its place is a usage error, not the value.
    let value = |args: &mut std::iter::Skip<std::env::Args>, flag: &str| match args.next() {
        Some(v) if !v.starts_with("--") => v,
        _ => {
            eprintln!("{flag} needs a value");
            std::process::exit(2);
        }
    };
    while let Some(a) = args.next() {
        match a.as_str() {
            "--profile" => profile = Some(value(&mut args, "--profile")),
            "--socket" => socket = Some(PathBuf::from(value(&mut args, "--socket"))),
            "--offline" => offline = true,
            "--agent-surface" => surface_file = Some(value(&mut args, "--agent-surface")),
            "--bench" => bench = true,
            "--diag" => {
                bench = true;
                diag_mode = true;
            }
            "--bench-tabs" => {
                bench = true;
                bench_tabs = true;
            }
            "--bench-motion" => {
                bench = true;
                bench_motion = true;
            }
            "--bench-panels" => {
                bench = true;
                bench_panels = true;
            }
            "--bench-history" => history_file = Some(PathBuf::from(value(&mut args, "--bench-history"))),
            "--prompt" => prompt = Some(value(&mut args, "--prompt")),
            "--refresh-hz" => refresh_hz = value(&mut args, "--refresh-hz").parse().ok(),
            "--accent" => accent = Some(value(&mut args, "--accent")),
            "--theme" => theme_flag = Some(value(&mut args, "--theme")),
            "--motion" => motion_flag = Some(value(&mut args, "--motion")),
            "--session" => start_session = Some(value(&mut args, "--session")),
            "--accent-picker" => open_picker = true,
            "--palette" => {
                open_palette = Some(match value(&mut args, "--palette").as_str() {
                    "goto" => palette::Mode::GoTo,
                    "commands" => palette::Mode::Commands,
                    v => {
                        eprintln!("--palette takes goto or commands: {v}");
                        std::process::exit(2);
                    }
                })
            }
            "--dash" => {
                let v = value(&mut args, "--dash");
                // A token on a command line is readable by every local user.
                if dash::flag_has_token(&v) {
                    eprintln!("{}", dash::FLAG_TOKEN);
                    std::process::exit(2);
                }
                dash_flag = Some(v);
            }
            "--size" => {
                let v = value(&mut args, "--size");
                let parsed = v.split_once('x').and_then(|(w, h)| Some((w.parse::<f32>().ok()?, h.parse::<f32>().ok()?)));
                match parsed.filter(|&(w, h)| (640.0..=8192.0).contains(&w) && (420.0..=8192.0).contains(&h)) {
                    Some(s) => window_size = Some(s),
                    None => {
                        eprintln!("--size takes WxH, at least 640x420: {v}");
                        std::process::exit(2);
                    }
                }
            }
            "-h" | "--help" => {
                let names: Vec<&str> = palettes::THEMES.iter().map(|t| t.0).collect();
                println!(
                    "synaps-desktop [--profile P | --socket PATH | --offline] [--session ID] [--theme NAME] [--accent #RRGGBB] [--accent-picker] [--palette goto|commands] [--size WxH] [--dash URL|FILE] [--motion full|reduced|off] [--bench [--prompt TEXT] [--refresh-hz HZ] | --bench-tabs [--bench-history FILE] | --bench-motion | --bench-panels]\n\
                     theme: --theme, else ${}, else the picker's settings file ({}), else `theme =` in the Synaps config, else {}\n\
                     themes: {}\n\
                     accent (overrides the theme's \"you\" and focus colours): --accent, else ${}, else the settings file, else none",
                    theme::ENV_THEME,
                    theme::settings_path().map(|p| p.display().to_string()).unwrap_or_else(|| "none".into()),
                    theme::DEFAULT_THEME,
                    names.join(" "),
                    theme::ENV_ACCENT,
                );
                return;
            }
            other => {
                eprintln!("unknown argument {other}");
                std::process::exit(2);
            }
        }
    }
    if bench {
        gpui_kit::profiler::set_trace_enabled(true);
    }
    let settings = theme::settings_path();
    let env_accent = std::env::var(theme::ENV_ACCENT).ok();
    let saved_accent = settings.as_deref().and_then(theme::load_accent);
    // A bench measures the default layout, whatever was folded last.
    let panels = if bench { theme::Panels::default() } else { settings.as_deref().map(theme::load_panels).unwrap_or_default() };
    let (accent, accent_source, mut warnings) = theme::resolve_accent(accent.as_deref(), env_accent.as_deref(), saved_accent.as_deref());
    // The theme: the Synaps config this profile reads, as the TUI does.
    let base = synaps_client::discovery::base_dir();
    let profile_name = profile.clone().or_else(|| std::env::var("SYNAPS_PROFILE").ok());
    let (config, config_skipped) = theme::synaps_theme(&base, profile_name.as_deref());
    let env_theme = std::env::var(theme::ENV_THEME).ok();
    let saved_theme = settings.as_deref().and_then(theme::load_theme);
    let (theme_name, raw, theme_source, theme_warnings) =
        theme::resolve_theme(theme_flag.as_deref(), env_theme.as_deref(), saved_theme.as_deref(), config, Some(&base));
    warnings.extend(theme_warnings);
    let follows_config = matches!(theme_source, theme::Source::Config | theme::Source::Default);
    if follows_config {
        warnings.extend(config_skipped);
    }
    // Motion: the TUI's switch, the flag, the saved choice; a bench reads
    // no saved setting (it measures the default).
    let saved_motion = if bench { None } else { settings.as_deref().and_then(|p| theme::load_str(p, "motion")) };
    let no_boot_fx = std::env::var(motion::ENV_NO_BOOT_FX).ok();
    let (motion_setting, motion_warning) = motion::Setting::resolve(motion_flag.as_deref(), no_boot_fx.as_deref(), saved_motion.as_deref());
    warnings.extend(motion_warning);
    for w in warnings {
        eprintln!("synaps-desktop: {w}");
    }
    let choice = theme::Choice { theme: theme_name, raw, accent, text: panels.text_size };
    // `--agent-surface`: a bounded local fixture, read only, on the desk's
    // theme, fonts and text size (the settings and the config above are
    // only read). Never the daemon, never a settings write.
    if let Some(path) = surface_file {
        let surface = agent_surface::Surface::read(&path).unwrap_or_else(|e| {
            eprintln!("agent surface: {e}");
            std::process::exit(2);
        });
        gpui_kit::application().with_assets(glyph::Sprites).run(move |cx| {
            let options = window_options(window_size, false, cx);
            agent_surface::launch(surface, motion_setting, choice, options, cx);
            cx.activate(true);
        });
        return;
    }
    // The synthetic transport, only for an interactive `--offline` window.
    let synthetic = (offline && !bench).then(demo::events);
    // In bench mode only an explicitly named daemon is ever contacted.
    let connect = !offline && (!bench || profile.is_some() || socket.is_some());
    // The open tabs are saved per daemon (the socket it resolves to), and
    // `--offline`'s apart; a bench neither reads nor writes them. Two
    // windows on one daemon: the last to save wins.
    let mut tabs_key = (offline && !bench).then(|| "offline".to_string());
    let mut legacy_key = None;
    // How Settings and Usage find the daemon's synaps-dash (`views::Launch`).
    let mut launch = views::Launch { dash_flag, ..Default::default() };
    // No --profile: `$SYNAPS_PROFILE`, else the default daemon (as the TUI).
    let client = if connect {
        let endpoint = match socket {
            Some(s) => Endpoint::Socket(s),
            None => Endpoint::profile(profile.as_deref()),
        };
        launch.endpoint = Some(endpoint.clone());
        if !bench {
            tabs_key = Some(format!("socket:{}", canonical_socket(&endpoint.resolve()).display()));
            // What builds before this one keyed them by (read once).
            legacy_key = Some(match &endpoint {
                Endpoint::Socket(s) => format!("socket:{}", s.display()),
                Endpoint::Profile(p) => format!("profile:{}", p.as_deref().unwrap_or("default")),
            });
        }
        match Client::spawn(ClientOptions::new(endpoint, Identity::desktop()).poll_sessions(Duration::from_secs(3))) {
            Ok(c) => Some(c),
            Err(e) => {
                eprintln!("synaps-desktop: cannot start the connection thread: {e}");
                None
            }
        }
    } else {
        None
    };

    let load = |k: Option<&str>| k.and_then(|k| settings.as_deref().and_then(|p| theme::load_tabs(p, k)));
    // The split layout first (split panes step 5): whole and still the
    // tabs' union, else the tabs as one pane (an older file, or one an
    // older build wrote since).
    // Split panes off, the layout is never read (Shady #55 n2); the file
    // is read once for both (n3).
    let split_env = settings_view::split_env();
    let (restore_layout, tabs) =
        tabs_key.as_deref().zip(settings.as_deref()).map(|(k, p)| theme::load_restore(p, k, split_env)).unwrap_or_default();
    let restore_tabs = tabs.or_else(|| load(legacy_key.as_deref()));
    // The favourites, under the same key (a bench has none).
    let favs = tabs_key.as_deref().zip(settings.as_deref()).map(|(k, p)| theme::load_favourites(p, k)).unwrap_or_default();
    if let (Some(k), Some(p)) = (tabs_key.as_deref(), settings.as_deref()) {
        theme::carry_favourites(p, k, &favs);
    }

    diag::mark("before_application");
    // The assets: the stroked glyphs' sprites (`glyph::Sprites`).
    gpui_kit::application().with_assets(glyph::Sprites).run(move |cx| {
        diag::mark("app_run_callback");
        house(motion_setting, choice, cx);
        app::bind_keys(cx);
        let mut options = window_options(window_size, bench, cx);
        if bench {
            // Measure the renderer, not the background throttles.
            options.inactive_frame_interval = None;
            options.low_power_frame_interval = None;
            options.thermal_frame_interval = None;
        }
        let (_, view) = beat::open_window(options, cx, beat::layered_by_env(), |window, cx| {
            cx.new(|cx| {
                let mut v = app::AppView::new(client, synthetic, window, cx);
                v.start_session = start_session;
                v.tabs_key = tabs_key;
                v.restore_tabs = restore_tabs;
                v.restore_layout = restore_layout;
                v.model.favs = favourites::Favourites::new(favs);
                v.picker_open = open_picker;
                if let Some(mode) = open_palette {
                    v.open_palette(mode, window, cx);
                }
                v.panels = panels;
                // OLDER left open: list up to the wire's cap from the start.
                v.model.named.deep = panels.history_open & rail::Bucket::Older.bit() != 0;
                v.model.named.deep_wanted = v.model.named.deep;
                v.accent_source = accent_source;
                v.theme_source = theme_source;
                v.theme_follows_config = follows_config;
                v.theme_profile = profile_name.clone();
                v.config_theme = v.config_choice().0.into();
                v.app_settings = settings_view::AppSettings::load(theme::settings_path().as_deref());
                v.usage = usage_view::UsageUi::new(v.app_settings.usage_host.clone(), v.app_settings.usage_range);
                v.dash.socket = launch.endpoint.as_ref().map(|e| e.resolve());
                v.dash.launch = launch;
                if !bench {
                    v.start_dash_poll(cx);
                }
                v
            })
        })
        .expect("failed to open the window");
        diag::mark("window_opened");
        if bench {
            let opts = bench::BenchOptions {
                t_main,
                exec_to_main_ms,
                refresh_hz,
                prompt: prompt.or(Some("Reply with one short sentence about Rust.".into())),
                stream_secs: 5.0,
                scroll_secs: 6.0,
                scroll_px_per_s: 4000.0,
                drag_secs: 2.5,
                history_messages: 2000,
                diag: diag_mode,
                tabs: bench_tabs,
                history_file,
                motion: bench_motion,
                panels: bench_panels,
            };
            let _ = cx.windows().first().and_then(|w| w.update(cx, |_, window, cx| bench::start(&view, opts, window, cx)).ok());
        }
        cx.activate(true);
    });
}

/// The house look on a new app, the desk's window and the `--agent-surface`
/// preview alike: the kit, the motion level, the kit's dark theme, the
/// embedded fonts, and the palette at its text size.
fn house(motion: motion::Setting, choice: theme::Choice, cx: &mut App) {
    gpui_kit::init(cx);
    diag::mark("kit_init_done");
    // After the kit read the OS's reduced-motion preference: a pinned
    // level wins over it (the kit's own motion follows ours).
    cx.set_global(motion);
    motion::Level::apply(cx);
    Theme::change(ThemeMode::Dark, None, cx);
    fonts::register(cx);
    theme::apply(choice, cx);
}

/// The window: `size` (else 1360×860), centred, as the app the desk is
/// (its app id, its title, its minimum).
fn window_options(window_size: Option<(f32, f32)>, bench: bool, cx: &mut App) -> WindowOptions {
    let (w, h) = window_size.unwrap_or((1360., 860.));
    let size = size(px(w), px(h));
    // A dialog-type window floats in tiling WMs (i3) instead of
    // re-tiling the user's workspace: benches and screenshot runs.
    let float = cfg!(target_os = "linux") && (bench || std::env::var_os("SYNAPS_DESKTOP_FLOAT").is_some());
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size, cx))),
        titlebar: Some(TitlebarOptions { title: Some("Synaps".into()), ..Default::default() }),
        app_id: Some("synaps-desktop".into()),
        window_min_size: Some(gpui_kit::size(px(640.), px(420.))),
        kind: if float { WindowKind::Dialog } else { WindowKind::Normal },
        ..Default::default()
    }
}

/// A daemon's socket path as one key, however it was reached: absolute,
/// its folder's symlinks resolved (the socket itself may not exist yet).
fn canonical_socket(p: &std::path::Path) -> PathBuf {
    let abs =
        if p.is_absolute() { p.to_path_buf() } else { std::env::current_dir().map(|d| d.join(p)).unwrap_or_else(|_| p.to_path_buf()) };
    match (abs.parent().and_then(|d| d.canonicalize().ok()), abs.file_name()) {
        (Some(dir), Some(name)) => dir.join(name),
        _ => abs,
    }
}
