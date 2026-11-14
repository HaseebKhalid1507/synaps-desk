//! Settings and Usage as tabs of the centre pane (`usage-design/spec.md`
//! section 1): the gear at the foot of the sessions rail and its menu, the
//! keys that open them, and the one channel both read, synaps-dash on the
//! daemon's machine ([`crate::dash`]): found, asked and polled here, off
//! the UI thread. A view repaints when an answer changes what it shows, on
//! a filter or value change, and under the pointer; nothing animates.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::app::{AppView, OpenSettings, OpenUsage, display, mono};
use crate::dash::{self, ConfigSnapshot, Fail, Source, Target};
use crate::tabs::Kind;
use crate::theme::Palette;
use crate::usage::{self, Board, Range, Raw};

/// The rail's foot (gear, today's spend).
pub(crate) const FOOT: f32 = 36.;
/// Usage asks again this often while its tab is on screen; the rail's
/// spend today, this often whatever is on screen.
const POLL: Duration = Duration::from_secs(60);

/// How this window reached its daemon (local or remote is decided from
/// it at each `Welcome`).
#[derive(Clone, Debug, Default)]
pub struct Launch {
    /// `--profile` (or the default daemon): the run dir's registry.
    pub profile_endpoint: Option<Option<String>>,
    /// `--socket PATH`.
    pub socket: Option<PathBuf>,
    /// `--dash URL`.
    pub dash_flag: Option<String>,
    /// The socket it resolves to (shown in CONNECTION).
    pub resolved: Option<PathBuf>,
}

/// A daemon is local when its endpoint came from the run-dir registry, or
/// when `--socket` is a registry entry's socket and the `Welcome`'s pid is
/// that entry's. Anything else (a forwarded socket) is remote.
pub fn is_local(launch: &Launch, registry: &[synaps_client::discovery::DaemonInfo], pid: u32) -> bool {
    if launch.profile_endpoint.is_some() {
        return true;
    }
    let Some(s) = launch.socket.as_deref() else { return false };
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    registry.iter().any(|d| d.pid == pid && canon(&d.socket_path()) == canon(s))
}

/// The dash as last asked.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Answer {
    #[default]
    Unknown,
    Up,
    Down(Fail),
}

/// The dash channel's state.
#[derive(Default)]
pub(crate) struct DashUi {
    pub launch: Launch,
    /// Where it is (`None`: nowhere to look; `Err`: a URL that is not one).
    pub found: Option<Result<(Target, Source), String>>,
    pub answer: Answer,
    pub config: Option<ConfigSnapshot>,
    /// Asked for `/api/config` and not answered yet.
    pub config_asking: bool,
    /// The daemon is local (`None`: not connected yet, or offline).
    pub local: Option<bool>,
    /// `summary.now.spend_today`, all hosts (the rail's foot).
    pub today: Option<f64>,
    /// The `daemon_gen` the channel was found for.
    found_gen: Option<u64>,
    pub poll: Option<Task<()>>,
}

impl DashUi {
    pub fn target(&self) -> Option<Target> {
        match &self.found {
            Some(Ok((t, _))) => Some(t.clone()),
            _ => None,
        }
    }

    /// CONFIG may be written: the dash answers, says the file is writable,
    /// and the daemon is on this machine (remote config is read-only in v1).
    pub fn config_writable(&self) -> bool {
        self.answer == Answer::Up && self.local == Some(true) && self.config.as_ref().is_some_and(|c| c.writable)
    }
}

/// One Usage load's outcome.
pub(crate) type Loaded = Result<(Arc<Board>, Option<f64>), Fail>;

/// Everything a Usage load asks, in order (blocking: a background thread).
pub(crate) fn load_usage(t: &Target, host: Option<&str>, range: Range, now_ms: i64, chips: &[String]) -> Loaded {
    let from = range.from(now_ms);
    let mut raw = Raw::default();
    for (key, path) in usage::paths(host, from) {
        match key {
            "summary" => raw.summary = dash::get_json(t, &path)?,
            "spend_host" => raw.spend_host = dash::get_json(t, &path)?,
            "spend_model" => raw.spend_model = dash::get_json(t, &path)?,
            "spend_role" => raw.spend_role = dash::get_json(t, &path)?,
            "agents" => raw.agents = dash::get_json(t, &path)?,
            "cache" => raw.cache = dash::get_json(t, &path)?,
            "latency" => raw.latency = dash::get_json(t, &path)?,
            "sessions" => raw.sessions = dash::get_json(t, &path)?,
            "heatmap" => raw.heatmap = dash::get_json(t, &path)?,
            "health" => raw.health = dash::get_json(t, &path)?,
            _ => {}
        }
    }
    // The chips: this load's host series, else the last ones known.
    let mut hosts: Vec<String> = raw.spend_host.series.iter().map(|s| s.key.clone()).filter(|k| k != "unknown").collect();
    if hosts.is_empty() {
        hosts = chips.to_vec();
    }
    hosts.sort();
    for h in hosts {
        let s: usage::Summary = dash::get_json(t, &usage::source_path(&h, from))?;
        raw.sources.push((h, s.all.last_ts));
    }
    // Today's spend for the rail: an unfiltered summary has it.
    let today = if host.is_none() {
        raw.summary.now.spend_today
    } else {
        dash::get_json::<usage::Summary>(t, "/api/telemetry/summary").ok().and_then(|s| s.now.spend_today)
    };
    Ok((Arc::new(Board::build(&raw, host, range, now_ms)), today))
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// The gear: a 6 px hollow square with a 2 px tooth centred on each side,
/// 12 px in all, drawn from rectangles like the state glyphs. `hover`: its
/// colour while the pointer is on the `gear` group.
pub(crate) fn gear(color: Hsla, hover: Option<Hsla>) -> Div {
    let tooth = |x: f32, y: f32| {
        div().absolute().left(px(x)).top(px(y)).size(px(2.)).bg(color).when_some(hover, |d, h| d.group_hover("gear", move |s| s.bg(h)))
    };
    div()
        .relative()
        .size(px(12.))
        .flex_none()
        .child(
            div()
                .absolute()
                .left(px(3.))
                .top(px(3.))
                .size(px(6.))
                .border_1()
                .border_color(color)
                .when_some(hover, |d, h| d.group_hover("gear", move |s| s.border_color(h))),
        )
        .child(tooth(5., 1.))
        .child(tooth(5., 9.))
        .child(tooth(1., 5.))
        .child(tooth(9., 5.))
}

/// Usage's tab glyph: three bars.
pub(crate) fn bars_glyph(color: Hsla) -> Div {
    let bar = |x: f32, h: f32| div().absolute().left(px(x)).bottom(px(1.)).w(px(2.)).h(px(h)).bg(color);
    div().relative().size(px(10.)).flex_none().child(bar(1., 4.)).child(bar(4., 8.)).child(bar(7., 6.))
}

/// The view keys: Ctrl+, Settings, Ctrl+Shift+U Usage (Cmd on macOS).
pub(crate) const fn view_keys(mac: bool) -> (&'static str, &'static str) {
    if mac { ("cmd-,", "cmd-shift-u") } else { ("ctrl-,", "ctrl-shift-u") }
}

/// How the menu writes them.
fn key_names() -> (&'static str, &'static str) {
    if cfg!(target_os = "macos") { ("Cmd+,", "Cmd+Shift+U") } else { ("Ctrl+,", "Ctrl+Shift+U") }
}

impl AppView {
    // ── opening the views ──────────────────────────────────────────────

    /// Open (or focus) Settings or Usage, at the end of the tab strip.
    pub(crate) fn open_view(&mut self, kind: Kind, cx: &mut Context<Self>) {
        self.gear_menu = false;
        if self.model.tabs.open_view(kind) {
            self.model.rail_gen += 1;
            self.reconcile(None);
        }
        self.view_shown(cx);
        cx.notify();
    }

    /// A view came on screen: what it shows is asked for again if stale.
    pub(crate) fn view_shown(&mut self, cx: &mut Context<Self>) {
        match self.model.tabs.view() {
            Some(Kind::Settings) => {
                self.settings.reference = None;
                self.dash_find(true);
                self.ask_config(cx);
            }
            Some(Kind::Usage) => {
                let stale = self.usage.fetched.is_none_or(|t| t.elapsed() >= POLL);
                if stale || self.usage.board.is_none() {
                    self.usage_load(cx);
                }
            }
            _ => {}
        }
    }

    // ── the channel ────────────────────────────────────────────────────

    /// Find the dash again: at a new `Welcome`, when the dash URL setting
    /// changed, or (`retry`) when it did not answer last time.
    pub(crate) fn dash_find(&mut self, retry: bool) {
        let g = self.model.daemon_gen;
        if self.dash.found_gen == Some(g) && !(retry && self.dash.answer != Answer::Up) {
            return;
        }
        self.dash.found_gen = Some(g);
        let run = synaps_client::discovery::run_dir();
        self.dash.local = self.model.daemon.as_ref().map(|d| {
            let reg = synaps_client::discovery::list_in(&run);
            is_local(&self.dash.launch, &reg, d.pid)
        });
        let profile =
            self.model.daemon.as_ref().and_then(|d| d.profile.clone()).or_else(|| self.dash.launch.profile_endpoint.clone().flatten());
        let env = std::env::var(dash::ENV_DASH).ok();
        let found = dash::discover(
            self.dash.local.unwrap_or(false),
            &run,
            profile.as_deref(),
            self.dash.launch.dash_flag.as_deref(),
            env.as_deref(),
            self.app_settings.dash_url.as_deref(),
        );
        if found != self.dash.found {
            self.dash.found = found;
            self.dash.answer = Answer::Unknown;
            self.dash.config = None;
        }
    }

    /// Find the dash again now (its URL setting changed).
    pub(crate) fn dash_refind(&mut self) {
        self.dash.found_gen = None;
        self.dash_find(false);
    }

    /// The channel answered (`Ok`) or not: what Settings and the rail say.
    /// A refusal (telemetry off, a 4xx) is an answer: the dash is up.
    fn dash_said(&mut self, r: Result<(), &Fail>) {
        self.dash.answer = match r {
            Err(e @ (Fail::Token | Fail::Unreachable(_))) => Answer::Down(e.clone()),
            _ => Answer::Up,
        };
        if matches!(r, Err(Fail::Token | Fail::Unreachable(_))) {
            // Settings falls back to its no-dash state; the token is new
            // at each daemon start, so look for it again next time.
            self.dash.config = None;
            self.dash.found_gen = None;
        }
    }

    /// `GET /api/config`.
    pub(crate) fn ask_config(&mut self, cx: &mut Context<Self>) {
        let Some(t) = self.dash.target() else { return };
        if self.dash.config_asking {
            return;
        }
        self.dash.config_asking = true;
        cx.spawn(async move |this, cx| {
            let got = cx.background_spawn(async move { dash::get_json::<ConfigSnapshot>(&t, "/api/config") }).await;
            let _ = this.update(cx, |v, cx| {
                v.dash.config_asking = false;
                match got {
                    Ok(c) => {
                        v.dash_said(Ok(()));
                        v.dash.config = Some(c);
                    }
                    Err(e) => v.dash_said(Err(&e)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// `POST /api/config {key, value}` (`None` unsets): the control shows
    /// the new value dimmed until the reply; the reply is the new snapshot,
    /// rendered as is; a refusal puts the dash's message under the row.
    pub(crate) fn config_write(&mut self, key: &'static str, value: Option<String>, cx: &mut Context<Self>) {
        let Some(t) = self.dash.target().filter(|_| self.dash.config_writable()) else { return };
        let shown = crate::settings::shown(key, value.as_deref());
        self.settings.writes.insert(key, crate::settings_view::Write::Sending(shown));
        cx.notify();
        let body = dash::config_body(key, value.as_deref());
        cx.spawn(async move |this, cx| {
            let got = cx
                .background_spawn(async move {
                    let text = dash::post_json(&t, "/api/config", &body)?;
                    serde_json::from_str::<ConfigSnapshot>(&text).map_err(|e| Fail::Bad(e.to_string()))
                })
                .await;
            let _ = this.update(cx, |v, cx| {
                match got {
                    Ok(c) => {
                        v.dash_said(Ok(()));
                        v.dash.config = Some(c);
                        v.settings.writes.remove(key);
                    }
                    Err(e) => {
                        if matches!(e, Fail::Token | Fail::Unreachable(_)) {
                            v.dash_said(Err(&e));
                        }
                        v.settings.writes.insert(key, crate::settings_view::Write::Refused(e.to_string()));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// A Usage load for the chips as they are now; a newer one replaces it.
    pub(crate) fn usage_load(&mut self, cx: &mut Context<Self>) {
        self.dash_find(true);
        let Some(t) = self.dash.target() else {
            self.usage.error = Some(match &self.dash.found {
                Some(Err(e)) => e.clone(),
                _ => "synaps-dash is not reachable: no URL for it".into(),
            });
            cx.notify();
            return;
        };
        self.usage.generation += 1;
        let (g, host, range, chips) = (self.usage.generation, self.usage.host.clone(), self.usage.range, self.usage.chips.clone());
        self.usage.loading = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let got = cx.background_spawn(async move { load_usage(&t, host.as_deref(), range, now_ms(), &chips) }).await;
            let _ = this.update(cx, |v, cx| {
                if v.usage.generation != g {
                    return;
                }
                v.usage.loading = false;
                v.usage.fetched = Some(std::time::Instant::now());
                match got {
                    Ok((board, today)) => {
                        v.dash_said(Ok(()));
                        if !board.hosts.is_empty() {
                            v.usage.chips = board.hosts.clone();
                        }
                        v.usage.error = None;
                        v.usage.hover = None;
                        v.usage.board = Some(board);
                        v.dash.today = today;
                    }
                    Err(e) => {
                        v.dash_said(Err(&e));
                        v.usage.error = Some(e.to_string());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Every [`POLL`]: Usage again while it is on screen; otherwise only the
    /// rail's spend today (one small request, and a frame only if it
    /// changed).
    pub(crate) fn start_dash_poll(&mut self, cx: &mut Context<Self>) {
        self.dash.poll = Some(cx.spawn(async move |this, cx| {
            loop {
                let job = this
                    .update(cx, |v, cx| {
                        v.dash_find(v.dash.answer != Answer::Up);
                        if v.model.tabs.view() == Some(Kind::Usage) {
                            v.usage_load(cx);
                            None
                        } else {
                            v.dash.target()
                        }
                    })
                    .ok();
                let Some(job) = job else { return };
                if let Some(t) = job {
                    let got = cx
                        .background_spawn(async move {
                            dash::get_json::<usage::Summary>(&t, "/api/telemetry/summary").map(|s| s.now.spend_today)
                        })
                        .await;
                    let _ = this.update(cx, |v, cx| {
                        let today = got.as_ref().ok().copied().flatten();
                        let before = v.dash.answer.clone();
                        match &got {
                            Ok(_) => v.dash_said(Ok(())),
                            Err(e) => v.dash_said(Err(e)),
                        }
                        let changed = today != v.dash.today || v.dash.answer != before;
                        v.dash.today = today;
                        if changed {
                            cx.notify();
                        }
                    });
                }
                cx.background_executor().timer(POLL).await;
            }
        }));
    }

    // ── the gear ───────────────────────────────────────────────────────

    /// The rail's foot: the gear (its menu), and today's spend (Usage).
    pub(crate) fn render_foot(&self, p: &Palette, cx: &Context<Self>) -> impl IntoElement {
        let on = self.model.tabs.view().is_some();
        let today = self.dash.today.filter(|_| self.dash.answer == Answer::Up);
        h_flex()
            .h(px(FOOT))
            .flex_none()
            .px(px(22.))
            .items_center()
            .border_t_1()
            .border_color(p.hair)
            .bg(p.chrome)
            .child(self.gear_button(on, p, cx))
            .child(div().flex_1())
            .when_some(today, |d, t| {
                d.child(
                    h_flex()
                        .id("foot-today")
                        .gap(px(8.))
                        .items_baseline()
                        .cursor_pointer()
                        .child(mono(usage::money(t), 12., p.cost_ch))
                        .child(display("T O D A Y", 12., p.dim_ch))
                        .tooltip(|window, cx| {
                            Tooltip::new("Spend today, all hosts · open Usage").action(&OpenUsage, None).build(window, cx)
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.open_view(Kind::Usage, cx))),
                )
            })
    }

    /// The folded rail's foot: the gear alone.
    pub(crate) fn render_strip_foot(&self, p: &Palette, cx: &Context<Self>) -> impl IntoElement {
        let on = self.model.tabs.view().is_some();
        v_flex()
            .w_full()
            .h(px(FOOT))
            .flex_none()
            .items_center()
            .justify_center()
            .border_t_1()
            .border_color(p.hair)
            .child(self.gear_button(on, p, cx))
    }

    fn gear_button(&self, on: bool, p: &Palette, cx: &Context<Self>) -> impl IntoElement {
        let (rest, hover) = (if on { p.you_bar } else { p.dim_ch }, if on { p.you_bar } else { p.prose_ch });
        div()
            .id("gear")
            .group("gear")
            .size(px(24.))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .child(gear(rest, Some(hover)))
            .tooltip(|window, cx| Tooltip::new("Settings and Usage").build(window, cx))
            .on_click(cx.listener(|this, ev: &ClickEvent, _, cx| {
                cx.stop_propagation();
                let dismissed = this.gear_dismissed.take();
                if let ClickEvent::Mouse(m) = ev
                    && dismissed == Some(m.down.position)
                {
                    return;
                }
                this.gear_menu = !this.gear_menu;
                cx.notify();
            }))
    }

    /// The gear's menu, above it on `card`: two rows with their keys.
    pub(crate) fn render_gear_menu(&self, folded: bool, p: &Palette, cx: &Context<Self>) -> impl IntoElement {
        let (ks, ku) = key_names();
        let row = |id: &'static str, label: &'static str, key: &'static str, kind: Kind| {
            h_flex()
                .id(id)
                .h(px(32.))
                .px(px(14.))
                .gap(px(24.))
                .items_center()
                .cursor_pointer()
                .hover(|d| d.bg(p.raised))
                .child(div().flex_1().text_size(px(14.)).text_color(p.ink).child(label))
                .child(mono(key, 12., p.dim_k))
                .on_click(cx.listener(move |this, _, _, cx| this.open_view(kind, cx)))
        };
        v_flex()
            .id("gear-menu")
            .absolute()
            .left(px(if folded { 6. } else { 14. }))
            .bottom(px(FOOT + 4.))
            .w(px(240.))
            .py(px(4.))
            .bg(p.card)
            .border_1()
            .border_color(p.edge)
            .occlude()
            .on_mouse_down_out(cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                this.gear_dismissed = Some(ev.position);
                this.gear_menu = false;
                cx.notify();
            }))
            .child(row("menu-settings", "Settings", ks, Kind::Settings))
            .child(row("menu-usage", "Usage", ku, Kind::Usage))
    }
}

/// The actions' fallbacks when the view does not have the focus (as the
/// tab keys': [`crate::app`]).
pub(crate) fn view_fallbacks(view: WeakEntity<AppView>, cx: &mut App) {
    let v = view.clone();
    cx.on_action(move |_: &OpenSettings, cx| {
        let _ = v.update(cx, |v, cx| v.open_view(Kind::Settings, cx));
    });
    cx.on_action(move |_: &OpenUsage, cx| {
        let _ = view.update(cx, |v, cx| v.open_view(Kind::Usage, cx));
    });
}

#[cfg(test)]
mod tests {
    // Not `super::*`: gpui_kit's `test` attribute would shadow the built-in.
    use super::{Launch, is_local, view_keys};
    use synaps_client::discovery::DaemonInfo;

    fn info(pid: u32, socket: &str) -> DaemonInfo {
        serde_json::from_value(serde_json::json!({"pid": pid, "protocol_version": 3, "socket": socket})).unwrap()
    }

    #[test]
    fn local_is_the_registry_or_its_socket_and_pid() {
        let reg = [info(7, "/run/x/daemon.sock")];
        let profile = Launch { profile_endpoint: Some(None), ..Default::default() };
        assert!(is_local(&profile, &[], 1), "the registry's endpoint");
        let same = Launch { socket: Some("/run/x/daemon.sock".into()), ..Default::default() };
        assert!(is_local(&same, &reg, 7));
        assert!(!is_local(&same, &reg, 8), "another pid behind the same path: a forward");
        let forwarded = Launch { socket: Some("/tmp/avante.sock".into()), ..Default::default() };
        assert!(!is_local(&forwarded, &reg, 7));
        assert!(!is_local(&Launch::default(), &reg, 7));
    }

    #[test]
    fn view_keys_parse() {
        for mac in [false, true] {
            let (a, b) = view_keys(mac);
            for k in [a, b] {
                gpui_kit::Keystroke::parse(k).unwrap_or_else(|e| panic!("{k}: {e:?}"));
            }
        }
    }
}
