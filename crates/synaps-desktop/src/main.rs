//! Synaps Desktop (GPUI spike): a thin client for the Synaps daemon.
//!
//! ```text
//! synaps-desktop [--profile P | --socket PATH | --offline] [--session ID] [--accent #RRGGBB] [--accent-picker]
//! synaps-desktop --bench [--profile P --prompt TEXT] [--refresh-hz HZ]
//! ```
//! `--bench` connects to a daemon only when `--profile` or `--socket` is
//! given (never the default daemon), runs the synthetic scenarios, prints
//! JSON and exits (watchdog: 30 s). `--offline` shows the synthetic
//! transport's sessions ([`demo`]). The accent: `--accent`, else
//! `$SYNAPS_DESKTOP_ACCENT`, else the settings file the in-app picker writes
//! ([`theme::settings_path`]), else Arasaka red.

// serde_json's `json!` in the synthetic transport and its tests nests deep.
#![recursion_limit = "256"]

mod app;
mod bench;
mod chrome;
mod demo;
mod diag;
mod files;
mod fonts;
mod mem;
mod model;
mod named;
mod stats;
mod theme;

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
    let mut bench = false;
    let mut diag_mode = false;
    let mut prompt: Option<String> = None;
    let mut refresh_hz: Option<f64> = None;
    let mut accent: Option<String> = None;
    let mut start_session: Option<String> = None;
    let mut open_picker = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--profile" => profile = args.next(),
            "--socket" => socket = args.next().map(PathBuf::from),
            "--offline" => offline = true,
            "--bench" => bench = true,
            "--diag" => {
                bench = true;
                diag_mode = true;
            }
            "--prompt" => prompt = args.next(),
            "--refresh-hz" => refresh_hz = args.next().and_then(|s| s.parse().ok()),
            "--accent" => accent = Some(args.next().unwrap_or_default()),
            "--session" => start_session = args.next(),
            "--accent-picker" => open_picker = true,
            "-h" | "--help" => {
                println!(
                    "synaps-desktop [--profile P | --socket PATH | --offline] [--session ID] [--accent #RRGGBB] [--accent-picker] [--bench [--prompt TEXT] [--refresh-hz HZ]]\n\
                     accent: --accent, else ${}, else the picker's settings file ({}), else {}",
                    theme::ENV_ACCENT,
                    theme::settings_path().map(|p| p.display().to_string()).unwrap_or_else(|| "none".into()),
                    theme::to_hex(theme::DEFAULT_ACCENT)
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
    let env_accent = std::env::var(theme::ENV_ACCENT).ok();
    let saved_accent = theme::settings_path().and_then(|p| theme::load_accent(&p));
    let (accent, _, warnings) = theme::resolve(accent.as_deref(), env_accent.as_deref(), saved_accent.as_deref());
    for w in warnings {
        eprintln!("synaps-desktop: {w}");
    }
    // The synthetic transport, only for an interactive `--offline` window.
    let synthetic = (offline && !bench).then(demo::events);
    // In bench mode only an explicitly named daemon is ever contacted.
    let connect = !offline && (!bench || profile.is_some() || socket.is_some());
    // No --profile: `$SYNAPS_PROFILE`, else the default daemon (as the TUI).
    let client = if connect {
        let endpoint = match socket {
            Some(s) => Endpoint::Socket(s),
            None => Endpoint::profile(profile.as_deref()),
        };
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

    diag::mark("before_application");
    gpui_kit::application().run(move |cx| {
        diag::mark("app_run_callback");
        gpui_kit::init(cx);
        diag::mark("kit_init_done");
        Theme::change(ThemeMode::Dark, None, cx);
        fonts::register(cx);
        theme::apply(accent, cx);
        let size = size(px(1360.), px(860.));
        let mut options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size, cx))),
            titlebar: Some(TitlebarOptions { title: Some("Synaps".into()), ..Default::default() }),
            app_id: Some("synaps-desktop".into()),
            window_min_size: Some(gpui_kit::size(px(640.), px(420.))),
            ..Default::default()
        };
        // A dialog-type window floats in tiling WMs (i3) instead of
        // re-tiling the user's workspace: benches and screenshot runs.
        #[cfg(target_os = "linux")]
        if bench || std::env::var_os("SYNAPS_DESKTOP_FLOAT").is_some() {
            options.kind = WindowKind::Dialog;
        }
        if bench {
            // Measure the renderer, not the background throttles.
            options.inactive_frame_interval = None;
            options.low_power_frame_interval = None;
            options.thermal_frame_interval = None;
        }
        let (_, view) = gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| {
                let mut v = app::AppView::new(client, synthetic, window, cx);
                v.start_session = start_session;
                v.picker_open = open_picker;
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
                history_messages: 2000,
                diag: diag_mode,
            };
            let _ = cx.windows().first().and_then(|w| w.update(cx, |_, window, cx| bench::start(&view, opts, window, cx)).ok());
        }
        cx.activate(true);
    });
}
