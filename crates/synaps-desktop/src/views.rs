//! Settings and Usage as tabs of the centre pane (`usage-design/spec.md`
//! section 1): the gear at the foot of the sessions rail and its menu, the
//! keys that open them, the cached panes they draw in ([`ViewPane`]), and
//! the one channel both read, synaps-dash on the daemon's machine
//! ([`crate::dash`]): found, asked and polled here, off the UI thread. A
//! view draws when an answer changes what it shows, on a filter or value
//! change, and under the pointer; nothing animates.
//!
//! Config is written only when every link is proven ([`DashUi::config_gate`]):
//! the daemon is on this machine by one rule for every launch
//! ([`locality`]), the dash came from that daemon's own URL file, and the
//! dash says it serves that daemon (`/api/info`'s pid and profile are the
//! `Welcome`'s). Anything unproven is read-only, with the reason shown.

use std::hash::{Hash, Hasher};
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use synaps_client::Endpoint;
use synaps_client::discovery::DaemonInfo;

use crate::app::{AppView, OpenSettings, OpenUsage, display, mono};
use crate::dash::{self, ConfigSnapshot, Fail, Info, Source, Target};
use crate::settings_view::Write;
use crate::tabs::Kind;
use crate::theme::Palette;
use crate::usage::{self, Board, Range, Raw};

/// The rail's foot (gear, today's spend).
pub(crate) const FOOT: f32 = 36.;
/// Usage asks again this often while its tab is on screen; the rail's
/// spend today, this often whatever is on screen (not while the window is
/// in the background).
const POLL: Duration = Duration::from_secs(60);
/// A whole Usage load's deadline (each request has its own,
/// [`dash::REQUEST`]).
const LOAD: Duration = Duration::from_secs(45);

/// How this window reached its daemon.
#[derive(Clone, Debug, Default)]
pub struct Launch {
    /// The endpoint the client connects to (`--socket`, `--profile`, the
    /// default daemon). Resolved again at each check: a registry entry can
    /// appear after launch.
    pub endpoint: Option<Endpoint>,
    /// `--dash URL|FILE` (never a token: [`dash::flag_target`]).
    pub dash_flag: Option<String>,
}

/// Is the daemon on this machine? One rule for every launch (`--profile`,
/// `$SYNAPS_PROFILE`, the default daemon, `--socket`): a registry entry in
/// this machine's run dir has the `Welcome`'s pid and names the socket this
/// window resolves to, and that pid is a daemon process here
/// (`daemon_here`). A forward at the conventional path, an empty or stale
/// registry, an unknown pid: not proven. `Err`: why, in one line.
pub fn locality(resolved: Option<&Path>, registry: &[DaemonInfo], pid: u32, daemon_here: impl Fn(u32) -> bool) -> Result<(), String> {
    if pid == 0 {
        return Err("the daemon did not say its pid".into());
    }
    let Some(socket) = resolved else { return Err("no socket to check".into()) };
    let entries: Vec<&DaemonInfo> = registry.iter().filter(|d| d.pid == pid).collect();
    if entries.is_empty() {
        return Err(format!("no registry entry on this machine has its pid {pid}"));
    }
    if !entries.iter().any(|d| same_path(&d.socket_path(), socket)) {
        return Err(format!("this machine's registry entry for pid {pid} names another socket"));
    }
    if !daemon_here(pid) {
        return Err(format!("pid {pid} is not a daemon process on this machine"));
    }
    Ok(())
}

fn same_path(a: &Path, b: &Path) -> bool {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    a == b || canon(a) == canon(b)
}

/// `pid` runs `… daemon …` here, as the dash checks its own parent. Off
/// Linux there is no cheap way to ask (and no dash: it needs `/proc`).
pub fn daemon_process(pid: u32) -> bool {
    if cfg!(target_os = "linux") {
        std::fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|c| c.split(|&b| b == 0).any(|a| a == b"daemon"))
    } else {
        true
    }
}

/// The dash as last asked.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Answer {
    #[default]
    Unknown,
    Up,
    Down(Fail),
}

/// Config requests in order (one channel, many clicks): one write in
/// flight, the latest value per key queued behind it, each request stamped
/// and a reply older than one already applied dropped, no read while a
/// write is out.
#[derive(Default)]
pub(crate) struct ConfigOrder {
    seq: u64,
    seen: u64,
    writing: Option<&'static str>,
    queued: Vec<(&'static str, Option<String>)>,
}

/// A write to send: its stamp, key and value (`None` unsets).
pub(crate) type Send = (u64, &'static str, Option<String>);

impl ConfigOrder {
    /// A write: `Some` to send now, `None` queued (replacing an older
    /// value for its key).
    pub fn write(&mut self, key: &'static str, value: Option<String>) -> Option<Send> {
        if self.writing.is_some() {
            self.queued.retain(|q| q.0 != key);
            self.queued.push((key, value));
            return None;
        }
        self.writing = Some(key);
        self.seq += 1;
        Some((self.seq, key, value))
    }

    /// A read's stamp; `None` while a write is out (its reply is newer).
    pub fn read(&mut self) -> Option<u64> {
        if self.writing.is_some() {
            return None;
        }
        self.seq += 1;
        Some(self.seq)
    }

    /// A reply stamped `seq` landed: apply it (it is the newest yet)?
    pub fn fresh(&mut self, seq: u64) -> bool {
        let fresh = seq > self.seen;
        self.seen = self.seen.max(seq);
        fresh
    }

    /// The write out was answered: whether a newer value for its key is
    /// queued (its row stays "sending"), and the next write to send.
    pub fn written(&mut self) -> (bool, Option<Send>) {
        let key = self.writing.take();
        let newer = key.is_some_and(|k| self.queued.iter().any(|q| q.0 == k));
        if self.queued.is_empty() {
            return (newer, None);
        }
        let (k, v) = self.queued.remove(0);
        (newer, self.write(k, v))
    }

    /// Another dash or daemon: every reply on its way is stale and the
    /// queue goes; the write out still ends with [`ConfigOrder::written`].
    pub fn reset(&mut self) {
        self.seen = self.seq;
        self.queued.clear();
    }

    /// The write just taken could not be sent: nothing is out.
    pub fn abandon(&mut self) {
        self.writing = None;
        self.queued.clear();
    }

    #[cfg(test)]
    pub fn writing(&self) -> bool {
        self.writing.is_some()
    }
}

/// The dash channel's state.
#[derive(Default)]
pub(crate) struct DashUi {
    pub launch: Launch,
    /// Where it is (`None`: nowhere to look; `Err`: a URL that is not one).
    pub found: Option<Result<(Target, Source), String>>,
    pub answer: Answer,
    pub config: Option<ConfigSnapshot>,
    /// What `/api/info` said the dash serves (`Err`: it did not say).
    pub serves: Option<Result<Info, String>>,
    /// The daemon is on this machine ([`locality`]; `Err`: why not; `None`:
    /// not connected, or not checked yet).
    pub local: Option<Result<(), String>>,
    /// The socket the endpoint resolved to at the last check.
    pub socket: Option<PathBuf>,
    /// `summary.now.spend_today`, all hosts (the rail's foot).
    pub today: Option<f64>,
    /// The `daemon_gen` the channel was found for, and the last one a
    /// `Welcome` started a find for (once per daemon, however it went).
    found_gen: Option<u64>,
    welcomed: Option<u64>,
    finding: bool,
    /// Asked for while a find ran (it runs again after this one).
    refind: bool,
    /// Asked for while a find, a read or a write ran: done after it.
    want_config: bool,
    want_usage: bool,
    config_asking: bool,
    today_asking: bool,
    pub order: ConfigOrder,
    /// A poll fell while the window was in the background: done on return.
    missed_tick: bool,
    pub poll: Option<Task<()>>,
}

impl DashUi {
    pub fn target(&self) -> Option<Target> {
        match &self.found {
            Some(Ok((t, _))) => Some(t.clone()),
            _ => None,
        }
    }

    pub fn source(&self) -> Option<&Source> {
        match &self.found {
            Some(Ok((_, s))) => Some(s),
            _ => None,
        }
    }

    pub fn is_local(&self) -> bool {
        matches!(self.local, Some(Ok(())))
    }

    /// May CONFIG be written to the daemon `daemon` (its `Welcome`'s pid
    /// and profile)? Only when every link is proven: the daemon is on this
    /// machine, the dash is that daemon's own (its URL file, and
    /// `/api/info` names that pid and profile), it answers, and it can lock
    /// the file. `Err`: the first link that is not, in one line.
    pub fn config_gate(&self, daemon: Option<(u32, Option<&str>)>) -> Result<(), String> {
        let Some((pid, profile)) = daemon else { return Err("not connected to a daemon".into()) };
        match &self.local {
            None => return Err("checking where the daemon runs".into()),
            Some(Err(why)) => return Err(format!("the daemon is not provably on this machine: {why}")),
            Some(Ok(())) => {}
        }
        match &self.found {
            Some(Ok((_, Source::UrlFile(_)))) => {}
            Some(Ok((_, src))) => return Err(format!("the dash is {}, not this daemon's own", src.describe())),
            _ => return Err("this daemon has no synaps-dash".into()),
        }
        match &self.serves {
            None => return Err("asking the dash which daemon it serves".into()),
            Some(Err(e)) => return Err(format!("the dash did not say which daemon it serves: {e}")),
            Some(Ok(i)) if !i.serves(pid, profile) => {
                return Err(format!(
                    "the dash serves pid {} ({}), not this daemon's pid {pid} ({})",
                    i.pid,
                    i.profile.as_deref().filter(|p| !p.is_empty()).unwrap_or("default"),
                    profile.filter(|p| !p.is_empty()).unwrap_or("default"),
                ));
            }
            Some(Ok(_)) => {}
        }
        if self.answer != Answer::Up {
            return Err("synaps-dash is not answering".into());
        }
        match &self.config {
            None => Err("the config is not read yet".into()),
            Some(c) if !c.writable => Err("synaps-dash cannot lock the file".into()),
            Some(_) => Ok(()),
        }
    }
}

/// One look at this machine, off the UI thread: where the daemon runs,
/// where its dash is, and (local) its config's REFERENCE values.
pub(crate) struct Probe {
    socket: Option<PathBuf>,
    local: Option<Result<(), String>>,
    found: Option<Result<(Target, Source), String>>,
    reference: Option<crate::settings::Reference>,
}

/// [`Probe`] for the daemon `daemon` (its `Welcome`'s pid and profile).
pub(crate) fn probe(launch: &Launch, daemon: Option<(u32, Option<String>)>, env: Option<&str>, setting: Option<&str>) -> Probe {
    let (run, base) = (synaps_client::discovery::run_dir(), synaps_client::discovery::base_dir());
    let socket = launch.endpoint.as_ref().map(Endpoint::resolve);
    probe_in(&run, &base, socket, launch.dash_flag.as_deref(), daemon, env, setting)
}

/// [`probe`] in run dir `run` and base `base`, for the socket the
/// endpoint resolved to.
fn probe_in(
    run: &Path,
    base: &Path,
    socket: Option<PathBuf>,
    flag: Option<&str>,
    daemon: Option<(u32, Option<String>)>,
    env: Option<&str>,
    setting: Option<&str>,
) -> Probe {
    let local = daemon.as_ref().map(|(pid, _)| locality(socket.as_deref(), &synaps_client::discovery::list_in(run), *pid, daemon_process));
    let here = matches!(local, Some(Ok(())));
    let profile = daemon.and_then(|d| d.1);
    let found = dash::discover(here, run, profile.as_deref(), flag, env, setting);
    let reference = here.then(|| crate::settings::read_reference(base, profile.as_deref()));
    Probe { socket, local, found, reference }
}

/// One Usage load's outcome.
pub(crate) type Loaded = Result<(Arc<Board>, Option<f64>), Fail>;

/// Everything a Usage load asks, in order, done by `by` (blocking: a
/// background thread).
pub(crate) fn load_usage(t: &Target, host: Option<&str>, range: Range, now_ms: i64, chips: &[String], by: Instant) -> Loaded {
    let from = range.from(now_ms);
    let mut raw = Raw::default();
    let get = |path: &str| dash::get(t, path, Some(by));
    fn de<T: serde::de::DeserializeOwned>(path: &str, body: Result<String, Fail>) -> Result<T, Fail> {
        serde_json::from_str(&body?).map_err(|e| Fail::Bad(format!("{path}: {e}")))
    }
    for (key, path) in usage::paths(host, from) {
        let body = get(&path);
        match key {
            "summary" => raw.summary = de(&path, body)?,
            "spend_host" => raw.spend_host = de(&path, body)?,
            "spend_model" => raw.spend_model = de(&path, body)?,
            "spend_role" => raw.spend_role = de(&path, body)?,
            "agents" => raw.agents = de(&path, body)?,
            "cache" => raw.cache = de(&path, body)?,
            "latency" => raw.latency = de(&path, body)?,
            "sessions" => raw.sessions = de(&path, body)?,
            "heatmap" => raw.heatmap = de(&path, body)?,
            "health" => raw.health = de(&path, body)?,
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
        let path = usage::source_path(&h, from);
        let s: usage::Summary = de(&path, get(&path))?;
        raw.sources.push((h, s.all.last_ts));
    }
    // Today's spend for the rail: an unfiltered summary has it.
    let today = if host.is_none() {
        raw.summary.now.spend_today
    } else {
        dash::get_json_by::<usage::Summary>(t, "/api/telemetry/summary", Some(by)).ok().and_then(|s| s.now.spend_today)
    };
    Ok((Arc::new(Board::build(&raw, host, range, now_ms)), today))
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

// ── the panes ─────────────────────────────────────────────────────────────

/// Settings or Usage, drawn from the AppView's state in a view of its own
/// that GPUI caches: an AppView frame (a session streaming behind it, the
/// rail's poll, the clock) reuses its last paint. It draws again when
/// notified ([`AppView::views_changed`], and [`AppView::view_inputs`] for
/// what it reads of the rest of the app), on a scroll inside it, or when
/// its bounds change.
pub(crate) struct ViewPane {
    app: WeakEntity<AppView>,
    kind: Kind,
}

impl ViewPane {
    pub fn new(app: WeakEntity<AppView>, kind: Kind) -> Self {
        ViewPane { app, kind }
    }
}

impl Render for ViewPane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(app) = self.app.upgrade() else { return div().into_any_element() };
        let cx: &App = cx;
        let p = crate::theme::palette(cx);
        let vc = ViewCx { view: self.app.clone(), cx };
        let a = app.read(cx);
        match self.kind {
            Kind::Settings => a.render_settings(a.view_width, &p, &vc),
            Kind::Usage => a.render_usage(a.view_width, &p, &vc),
            Kind::Session => div().into_any_element(),
        }
    }
}

/// What a pane's render has in place of the AppView's `Context`: the app,
/// read-only, and listeners that run on the AppView.
pub(crate) struct ViewCx<'a> {
    view: WeakEntity<AppView>,
    cx: &'a App,
}

impl Deref for ViewCx<'_> {
    type Target = App;

    fn deref(&self) -> &App {
        self.cx
    }
}

impl ViewCx<'_> {
    /// As `Context::listener`, on the AppView.
    pub fn listener<E: ?Sized>(
        &self,
        f: impl Fn(&mut AppView, &E, &mut Window, &mut Context<AppView>) + 'static,
    ) -> impl Fn(&E, &mut Window, &mut App) + 'static {
        let view = self.view.clone();
        move |e, window, cx| {
            let _ = view.update(cx, |v, cx| f(v, e, window, cx));
        }
    }
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

/// The view keys: Ctrl+, Settings, Ctrl+Shift+U Usage (Cmd on macOS), and
/// Ctrl+Alt+U for Usage off macOS: IBus takes Ctrl+Shift+U for Unicode
/// entry while a text field has the focus.
pub(crate) const fn view_keys(mac: bool) -> (&'static str, &'static str, Option<&'static str>) {
    if mac { ("cmd-,", "cmd-shift-u", None) } else { ("ctrl-,", "ctrl-shift-u", Some("ctrl-alt-u")) }
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
        cx.notify();
    }

    /// The pane `kind` draws in.
    pub(crate) fn pane(&self, kind: Kind) -> &Entity<ViewPane> {
        if kind == Kind::Usage { &self.usage_pane } else { &self.settings_pane }
    }

    /// What Settings or Usage shows changed: its pane draws again (and the
    /// AppView, for the rail's foot and the tab strip).
    pub(crate) fn views_changed(&mut self, cx: &mut Context<Self>) {
        for p in [self.settings_pane.clone(), self.usage_pane.clone()] {
            p.update(cx, |_, cx| cx.notify());
        }
        cx.notify();
    }

    /// What the visible view reads of the app outside its own state (the
    /// daemon, the current session, the panels, the live sessions), as one
    /// number: when it changes, the cached pane draws again. Nothing in it
    /// changes at a stream's rate.
    pub(crate) fn view_inputs(&self, kind: Kind) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (kind as u8, self.model.daemon_gen, self.synthetic).hash(&mut h);
        self.model.daemon.as_ref().map(|d| (&d.version, &d.profile, d.pid)).hash(&mut h);
        match kind {
            Kind::Settings => {
                self.model.active.as_ref().map(|a| &a.session_id).hash(&mut h);
                self.model.tabs.current().map(|t| (t.id, &t.label)).hash(&mut h);
                (self.model.transcript.title(), self.model.active_name(), &self.model.stats.view).hash(&mut h);
                (self.panels, self.theme_source, self.accent_source, self.theme_follows_config).hash(&mut h);
            }
            Kind::Usage => self.model.sessions.iter().for_each(|s| s.id.hash(&mut h)),
            Kind::Session => {}
        }
        h.finish()
    }

    /// A view came on screen or a tab closed (after the frame that showed
    /// it, never inside a render): what a closed view showed goes, and what
    /// the one on screen shows is asked for again if stale.
    pub(crate) fn view_shown(&mut self, cx: &mut Context<Self>) {
        let open = |k: Kind| self.model.tabs.iter().any(|t| t.kind == k);
        if !open(Kind::Usage) {
            let u = &mut self.usage;
            (u.board, u.error, u.fetched, u.hover) = (None, None, None, None);
        }
        if !open(Kind::Settings) {
            self.settings.reference = None;
            self.settings.writes.clear();
            self.dash.config = None;
            self.dash.serves = None;
        }
        match self.model.tabs.view() {
            // Each opening reads the daemon's place, URL file and config again.
            Some(Kind::Settings) => {
                self.dash.found_gen = None;
                self.dash.want_config = true;
                self.dash_find(cx);
            }
            Some(Kind::Usage) => {
                let stale = self.usage.fetched.is_none_or(|t| t.elapsed() >= POLL);
                if stale || self.usage.board.is_none() {
                    self.usage_load(cx);
                }
            }
            _ => {}
        }
        self.views_changed(cx);
    }

    // ── the channel ────────────────────────────────────────────────────

    fn daemon_id(&self) -> Option<(u32, Option<&str>)> {
        self.model.daemon.as_ref().map(|d| (d.pid, d.profile.as_deref()))
    }

    /// May CONFIG be written ([`DashUi::config_gate`])? `Err`: why not.
    pub(crate) fn config_gate(&self) -> Result<(), String> {
        self.dash.config_gate(self.daemon_id())
    }

    /// The channel must be found (again) first: never yet for this daemon,
    /// or (`retry`) it did not answer last time.
    fn dash_stale(&self, retry: bool) -> bool {
        self.dash.finding || self.dash.found_gen != Some(self.model.daemon_gen) || (retry && matches!(self.dash.answer, Answer::Down(_)))
    }

    /// Look again for the daemon's place and its dash ([`probe`], off the
    /// UI thread): at a new `Welcome`, when the dash URL setting changed,
    /// when Settings opens, or when the dash did not answer. What waited on
    /// it runs after ([`AppView::dash_found`]). Never for `--offline`.
    pub(crate) fn dash_find(&mut self, cx: &mut Context<Self>) {
        if self.synthetic {
            return;
        }
        if self.dash.finding {
            self.dash.refind = true;
            return;
        }
        self.dash.finding = true;
        let g = self.model.daemon_gen;
        let launch = self.dash.launch.clone();
        let daemon = self.model.daemon.as_ref().map(|d| (d.pid, d.profile.clone()));
        let setting = self.app_settings.dash_url.clone();
        cx.spawn(async move |this, cx| {
            let p = cx
                .background_spawn(async move {
                    let env = std::env::var(dash::ENV_DASH).ok();
                    probe(&launch, daemon, env.as_deref(), setting.as_deref())
                })
                .await;
            let _ = this.update(cx, |v, cx| {
                v.dash.finding = false;
                // What it looked at changed meanwhile: look again.
                if v.model.daemon_gen != g || std::mem::take(&mut v.dash.refind) {
                    return v.dash_find(cx);
                }
                v.dash.found_gen = Some(g);
                v.dash_found(p, cx);
            });
        })
        .detach();
    }

    /// A Welcome came (the daemon is new, or back): find its channel, once
    /// (a dash that fails is looked for again by the poll, not each batch).
    pub(crate) fn dash_on_welcome(&mut self, cx: &mut Context<Self>) {
        let g = self.model.daemon_gen;
        if self.model.daemon.is_some() && self.dash.welcomed != Some(g) {
            self.dash.welcomed = Some(g);
            self.dash_find(cx);
        }
    }

    /// Find the dash again now (its URL setting changed).
    pub(crate) fn dash_refind(&mut self, cx: &mut Context<Self>) {
        self.dash.found_gen = None;
        self.dash.want_config = true;
        self.dash_find(cx);
    }

    /// A probe's answer: another dash or daemon drops all the old one said;
    /// then what waited on it.
    fn dash_found(&mut self, p: Probe, cx: &mut Context<Self>) {
        let moved = p.found != self.dash.found;
        if moved || p.local != self.dash.local {
            self.dash.config = None;
            self.dash.serves = None;
            self.dash.answer = Answer::Unknown;
            self.dash.order.reset();
            self.settings.writes.clear();
        }
        if moved {
            // Another store's numbers never stand under this one's header.
            let u = &mut self.usage;
            (u.board, u.error, u.fetched, u.hover) = (None, None, None, None);
            u.generation += 1;
            self.dash.today = None;
        }
        if matches!(self.dash.answer, Answer::Down(_)) {
            // Asked again below.
            self.dash.answer = Answer::Unknown;
        }
        (self.dash.found, self.dash.local, self.settings.reference) = (p.found, p.local, p.reference);
        if p.socket.is_some() {
            self.dash.socket = p.socket;
        }
        let view = self.model.tabs.view();
        if std::mem::take(&mut self.dash.want_config) || view == Some(Kind::Settings) {
            self.ask_config(cx);
        }
        if std::mem::take(&mut self.dash.want_usage) || (view == Some(Kind::Usage) && self.usage.board.is_none()) {
            self.usage_load(cx);
        } else {
            self.ask_today(cx);
        }
        self.views_changed(cx);
    }

    /// The channel answered (`Ok`) or not: what Settings and the rail say.
    /// A refusal (telemetry off, a 4xx) is an answer: the dash is up.
    fn dash_said(&mut self, r: Result<(), &Fail>) {
        let down = matches!(r, Err(Fail::Token | Fail::NoToken | Fail::Unreachable(_)));
        self.dash.answer = match r {
            Err(e) if down => Answer::Down(e.clone()),
            _ => Answer::Up,
        };
        if down {
            // Settings falls back to its no-dash state; the token is new
            // at each daemon start, so look for it again next time.
            self.dash.config = None;
            self.dash.serves = None;
            self.dash.found_gen = None;
        }
    }

    /// A failure as the views say it: a URL with no token says where one
    /// comes from.
    pub(crate) fn fail_text(&self, e: &Fail) -> String {
        match (e, self.dash.source()) {
            (Fail::NoToken, Some(Source::Setting)) => {
                "dash_url has no token: paste the URL with ?token= (kept for this run) or set $SYNAPS_DASH_URL".into()
            }
            (Fail::NoToken, Some(Source::Flag)) => {
                "--dash has no token: pass the dash's URL file (--dash FILE) or set $SYNAPS_DASH_URL".into()
            }
            _ => e.to_string(),
        }
    }

    /// `GET /api/info` and `GET /api/config`: which daemon the dash serves,
    /// and the config. Not while a write is out (its reply is newer). Not
    /// from another dash than a local daemon's own (`--dash` pointed at
    /// another machine's): Settings then shows the local file, read-only.
    pub(crate) fn ask_config(&mut self, cx: &mut Context<Self>) {
        if self.dash_stale(false) {
            self.dash.want_config = true;
            return self.dash_find(cx);
        }
        let Some(t) = self.dash.target() else { return };
        if self.dash.is_local() && !matches!(self.dash.source(), Some(Source::UrlFile(_))) {
            return;
        }
        if self.dash.config_asking {
            self.dash.want_config = true;
            return;
        }
        let Some(seq) = self.dash.order.read() else {
            self.dash.want_config = true;
            return;
        };
        self.dash.config_asking = true;
        cx.spawn(async move |this, cx| {
            let (info, got) = cx
                .background_spawn(
                    async move { (dash::get_json::<Info>(&t, "/api/info"), dash::get_json::<ConfigSnapshot>(&t, "/api/config")) },
                )
                .await;
            let _ = this.update(cx, |v, cx| {
                v.dash.config_asking = false;
                if v.dash.order.fresh(seq) {
                    v.dash.serves = Some(info.map_err(|e| v.fail_text(&e)));
                    match got {
                        Ok(c) => {
                            v.dash_said(Ok(()));
                            v.dash.config = Some(c);
                        }
                        Err(e) => v.dash_said(Err(&e)),
                    }
                }
                if std::mem::take(&mut v.dash.want_config) {
                    v.ask_config(cx);
                }
                v.views_changed(cx);
            });
        })
        .detach();
    }

    /// `POST /api/config {key, value}` (`None` unsets), when the gate is
    /// open: the control shows the new value dimmed until the reply; the
    /// reply is the new snapshot, rendered as is; a refusal puts the dash's
    /// message under the row. One write at a time, in the order made.
    pub(crate) fn config_write(&mut self, key: &'static str, value: Option<String>, cx: &mut Context<Self>) {
        if self.config_gate().is_err() {
            return;
        }
        self.settings.writes.insert(key, Write::Sending(crate::settings::shown(key, value.as_deref())));
        if let Some(send) = self.dash.order.write(key, value) {
            self.send_write(send, cx);
        }
        self.views_changed(cx);
    }

    fn send_write(&mut self, (seq, key, value): Send, cx: &mut Context<Self>) {
        let Some(t) = self.dash.target() else { return self.dash.order.abandon() };
        let body = dash::config_body(key, value.as_deref());
        cx.spawn(async move |this, cx| {
            let got = cx
                .background_spawn(async move {
                    let text = dash::post_json(&t, "/api/config", &body)?;
                    serde_json::from_str::<ConfigSnapshot>(&text).map_err(|e| Fail::Bad(e.to_string()))
                })
                .await;
            let _ = this.update(cx, |v, cx| {
                let (newer, next) = v.dash.order.written();
                if v.dash.order.fresh(seq) {
                    match got {
                        Ok(c) => {
                            v.dash_said(Ok(()));
                            v.dash.config = Some(c);
                            if !newer {
                                v.settings.writes.remove(key);
                            }
                        }
                        Err(e) => {
                            if matches!(e, Fail::Token | Fail::NoToken | Fail::Unreachable(_)) {
                                v.dash_said(Err(&e));
                            }
                            if !newer {
                                v.settings.writes.insert(key, Write::Refused(v.fail_text(&e)));
                            }
                        }
                    }
                }
                match next {
                    Some(n) if v.config_gate().is_ok() => v.send_write(n, cx),
                    Some(_) => {
                        v.dash.order.abandon();
                        v.settings.writes.retain(|_, w| !matches!(w, Write::Sending(_)));
                    }
                    None if std::mem::take(&mut v.dash.want_config) => v.ask_config(cx),
                    None => {}
                }
                v.views_changed(cx);
            });
        })
        .detach();
    }

    /// A Usage load for the chips as they are now. One at a time: a newer
    /// selection waits for the one running, then loads.
    pub(crate) fn usage_load(&mut self, cx: &mut Context<Self>) {
        if self.synthetic {
            self.usage.error = Some("offline: no synaps-dash".into());
            return self.views_changed(cx);
        }
        if self.dash_stale(true) {
            self.dash.want_usage = true;
            return self.dash_find(cx);
        }
        let Some(t) = self.dash.target() else {
            self.usage.error = Some(match &self.dash.found {
                Some(Err(e)) => e.clone(),
                _ => "synaps-dash is not reachable: no URL for it".into(),
            });
            return self.views_changed(cx);
        };
        if self.usage.loading {
            self.usage.queued = true;
            return;
        }
        let (g, host, range, chips) = (self.usage.generation, self.usage.host.clone(), self.usage.range, self.usage.chips.clone());
        self.usage.loading = true;
        self.views_changed(cx);
        let want = (host.clone(), range);
        cx.spawn(async move |this, cx| {
            let by = Instant::now() + LOAD;
            let got = cx.background_spawn(async move { load_usage(&t, host.as_deref(), range, now_ms(), &chips, by) }).await;
            let _ = this.update(cx, |v, cx| {
                v.usage.loading = false;
                // Another dash, or the chips moved meanwhile: not this one's.
                let current = v.usage.generation == g && (v.usage.host.clone(), v.usage.range) == want;
                if current {
                    v.usage.fetched = Some(Instant::now());
                    match got {
                        Ok((board, today)) => {
                            v.dash_said(Ok(()));
                            if !board.hosts.is_empty() {
                                v.usage.chips = board.hosts.clone();
                                // A saved host the store no longer has: ALL.
                                if v.usage.host.as_ref().is_some_and(|h| !board.hosts.contains(h)) {
                                    v.usage.host = None;
                                    v.usage.queued = true;
                                }
                            }
                            v.usage.error = None;
                            v.usage.hover = None;
                            v.usage.clear_paths();
                            v.usage.board = Some(board);
                            v.dash.today = today;
                        }
                        Err(e) => {
                            v.dash_said(Err(&e));
                            v.usage.error = Some(v.fail_text(&e));
                        }
                    }
                }
                if std::mem::take(&mut v.usage.queued) || !current {
                    v.usage_load(cx);
                }
                v.views_changed(cx);
            });
        })
        .detach();
    }

    /// The rail's spend today: one small request, and a frame only if what
    /// it shows changed.
    fn ask_today(&mut self, cx: &mut Context<Self>) {
        let Some(t) = self.dash.target() else {
            if self.dash.today.take().is_some() {
                cx.notify();
            }
            return;
        };
        if self.dash.today_asking {
            return;
        }
        self.dash.today_asking = true;
        cx.spawn(async move |this, cx| {
            let got = cx
                .background_spawn(async move { dash::get_json::<usage::Summary>(&t, "/api/telemetry/summary").map(|s| s.now.spend_today) })
                .await;
            let _ = this.update(cx, |v, cx| {
                v.dash.today_asking = false;
                let today = got.as_ref().ok().copied().flatten();
                let before = v.dash.answer.clone();
                match &got {
                    Ok(_) => v.dash_said(Ok(())),
                    Err(e) => v.dash_said(Err(e)),
                }
                if v.dash.answer != before {
                    v.views_changed(cx);
                } else if today != v.dash.today {
                    cx.notify();
                }
                v.dash.today = today;
            });
        })
        .detach();
    }

    /// Every [`POLL`]: Usage again while it is on screen, else the rail's
    /// spend today; nothing while the window is in the background (done on
    /// its return) or for `--offline`. The first find is the `Welcome`'s.
    pub(crate) fn start_dash_poll(&mut self, cx: &mut Context<Self>) {
        if self.synthetic {
            return;
        }
        self.dash.poll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                if this.update(cx, |v, cx| v.dash_tick(cx)).is_err() {
                    return;
                }
            }
        }));
    }

    fn dash_tick(&mut self, cx: &mut Context<Self>) {
        if !self.window_active {
            self.dash.missed_tick = true;
            return;
        }
        if self.dash_stale(true) {
            return self.dash_find(cx);
        }
        if self.model.tabs.view() == Some(Kind::Usage) {
            if !self.usage.loading {
                self.usage_load(cx);
            }
        } else {
            self.ask_today(cx);
        }
    }

    /// The window came to the front: a poll it missed runs now.
    pub(crate) fn window_activated(&mut self, active: bool, cx: &mut Context<Self>) {
        self.window_active = active;
        if active && std::mem::take(&mut self.dash.missed_tick) {
            self.dash_tick(cx);
        }
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
    use super::{Answer, ConfigOrder, DashUi, locality, probe_in, view_keys};
    use crate::dash::{ConfigSnapshot, Fail, Info, Source, Target};
    use std::path::{Path, PathBuf};
    use synaps_client::discovery::DaemonInfo;

    fn info(pid: u32, socket: &str) -> DaemonInfo {
        serde_json::from_value(serde_json::json!({"pid": pid, "protocol_version": 3, "socket": socket})).unwrap()
    }

    const YES: fn(u32) -> bool = |_| true;

    /// H2: one rule for every launch kind: a registry entry with the
    /// Welcome's pid and the resolved socket, of a daemon process here.
    #[test]
    fn local_is_a_registry_entry_with_this_pid_and_socket() {
        let sock = Path::new("/run/x/daemon.sock");
        let reg = [info(7, "/run/x/daemon.sock")];
        assert_eq!(locality(Some(sock), &reg, 7, YES), Ok(()));
        // An empty registry is never local, whatever the launch was.
        assert!(locality(Some(sock), &[], 7, YES).unwrap_err().contains("no registry entry"));
        // The ssh-forward recipe: a forward at the conventional path. The
        // remote's pid is in no entry here (or a stale entry has another pid).
        assert!(locality(Some(sock), &[info(4242, "/run/x/daemon.sock")], 7, YES).unwrap_err().contains("pid 7"));
        // The pid is a registry's, but for another socket (a forward placed elsewhere).
        let fwd = Path::new("/tmp/avante.sock");
        assert!(locality(Some(fwd), &reg, 7, YES).unwrap_err().contains("another socket"));
        // A stale entry whose pid happens to match: not a daemon process here.
        assert!(locality(Some(sock), &reg, 7, |_| false).unwrap_err().contains("not a daemon process"));
        // An older daemon that says no pid.
        assert!(locality(Some(sock), &[info(0, "/run/x/daemon.sock")], 0, YES).is_err());
        assert!(locality(None, &reg, 7, YES).is_err());
    }

    /// The probe on a real run dir: a forward put at the conventional
    /// `run/daemon-p.sock` with no registry entry (the spec's ssh recipe;
    /// with the remote's URL file copied next to it) is remote, and neither
    /// that URL file nor the local config is read; with this process's own
    /// entry for that socket it is proven only if the pid is a daemon here.
    #[test]
    fn a_forward_at_the_conventional_path_is_remote() {
        let dir = std::env::temp_dir().join(format!("sd-probe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("synaps-dash-p.url"), "http://127.0.0.1:7801/?token=ab\n").unwrap();
        let sock = dir.join("daemon-p.sock");
        let me = std::process::id();
        let probe = |flag: Option<&str>| probe_in(&dir, &dir, Some(sock.clone()), flag, Some((me, Some("p".into()))), None, None);
        let p = probe(None);
        assert!(p.local.as_ref().unwrap().as_ref().unwrap_err().contains("no registry entry"), "{:?}", p.local);
        assert!(p.found.is_none() && p.reference.is_none(), "a remote daemon's URL file and config are never read");
        // `--dash` still serves Usage for it (read-only: the gate's source check).
        assert!(matches!(probe(Some("http://127.0.0.1:7717")).found, Some(Ok((_, Source::Flag)))));
        // An empty registry dir: the same.
        std::fs::write(dir.join("daemon-q.json"), "not json").unwrap();
        assert!(probe(None).local.unwrap().is_err());
        // This process's own entry for that socket.
        let entry = serde_json::json!({"pid": me, "protocol_version": 3, "profile": "p", "socket": sock});
        std::fs::write(dir.join("daemon-p.json"), entry.to_string()).unwrap();
        let p = probe(None);
        if cfg!(target_os = "linux") {
            // The test binary is no `daemon` process: proven not local.
            assert!(p.local.unwrap().unwrap_err().contains("not a daemon process"));
        } else {
            assert_eq!(p.local, Some(Ok(())));
            assert!(matches!(p.found, Some(Ok((_, Source::UrlFile(_))))));
            assert!(p.reference.unwrap().values.unwrap().is_empty());
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    fn target(url: &str) -> Target {
        Target::parse(url).unwrap()
    }

    /// A dash every link of which is proven: the gate is open.
    fn proven() -> DashUi {
        DashUi {
            local: Some(Ok(())),
            found: Some(Ok((target("http://127.0.0.1:7801/?token=ab"), Source::UrlFile(PathBuf::from("/run/synaps-dash-p.url"))))),
            serves: Some(Ok(Info { pid: 7, profile: Some("p".into()) })),
            answer: Answer::Up,
            config: Some(ConfigSnapshot { writable: true, ..Default::default() }),
            ..Default::default()
        }
    }

    /// H1 + H2: config is writable only when every link is proven; each
    /// unproven link says so.
    #[test]
    fn config_is_writable_only_when_every_link_is_proven() {
        let me = Some((7, Some("p")));
        assert_eq!(proven().config_gate(me), Ok(()));
        let why = |d: DashUi, daemon: Option<(u32, Option<&str>)>| d.config_gate(daemon).unwrap_err();
        assert!(why(proven(), None).contains("not connected"));
        // The daemon is not provably here.
        assert!(why(DashUi { local: None, ..proven() }, me).contains("checking"));
        assert!(
            why(DashUi { local: Some(Err("no registry entry on this machine has its pid 7".into())), ..proven() }, me)
                .contains("not provably on this machine: no registry entry")
        );
        // A local daemon with `--dash` (avante's dash from bella): read-only, always.
        let flag = DashUi { found: Some(Ok((target("http://127.0.0.1:7717/?token=ab"), Source::Flag))), ..proven() };
        assert_eq!(why(flag, me), "the dash is --dash, not this daemon's own");
        for src in [Source::Env, Source::Setting] {
            let d = DashUi { found: Some(Ok((target("http://127.0.0.1:7717/?token=ab"), src))), ..proven() };
            assert!(why(d, me).contains("not this daemon's own"));
        }
        assert!(why(DashUi { found: None, ..proven() }, me).contains("no synaps-dash"));
        // The URL file's dash serves another daemon (a copied URL file, a
        // stale one), or does not say.
        let other = DashUi { serves: Some(Ok(Info { pid: 8, profile: Some("p".into()) })), ..proven() };
        assert_eq!(why(other, me), "the dash serves pid 8 (p), not this daemon's pid 7 (p)");
        let profile = DashUi { serves: Some(Ok(Info { pid: 7, profile: None })), ..proven() };
        assert!(why(profile, me).contains("(default), not this daemon's pid 7 (p)"));
        assert!(why(DashUi { serves: None, ..proven() }, me).contains("asking"));
        assert!(why(DashUi { serves: Some(Err("401".into())), ..proven() }, me).contains("did not say"));
        // Down, not read, or no lock.
        assert!(why(DashUi { answer: Answer::Down(Fail::Token), ..proven() }, me).contains("not answering"));
        assert!(why(DashUi { config: None, ..proven() }, me).contains("not read"));
        assert!(why(DashUi { config: Some(ConfigSnapshot::default()), ..proven() }, me).contains("cannot lock"));
        // The default daemon: no profile on either side.
        let default = DashUi { serves: Some(Ok(Info { pid: 7, profile: None })), ..proven() };
        assert_eq!(default.config_gate(Some((7, None))), Ok(()));
        assert_eq!(
            DashUi { serves: Some(Ok(Info { pid: 7, profile: Some(String::new()) })), ..proven() }.config_gate(Some((7, None))),
            Ok(())
        );
        // A pid of 0 (an older daemon) never matches.
        let zero = DashUi { serves: Some(Ok(Info { pid: 0, profile: None })), ..proven() };
        assert!(zero.config_gate(Some((0, None))).is_err());
    }

    /// M2: one write in flight, the latest value per key queued, replies
    /// applied in request order, no read while a write is out.
    #[test]
    fn config_requests_apply_in_the_order_made() {
        let mut o = ConfigOrder::default();
        let get = o.read().unwrap();
        // Two quick picks on one key, and another key.
        let first = o.write("cache_ttl", Some("1h".into())).unwrap();
        assert!(o.write("cache_ttl", Some("5m".into())).is_none() && o.write("api_retries", Some("4".into())).is_none());
        assert!(o.write("cache_ttl", Some("hybrid".into())).is_none(), "replaces the queued 5m");
        assert!(o.read().is_none() && o.writing(), "no read while a write is out");
        // The write's reply lands first, then the older read's: dropped.
        assert!(o.fresh(first.0));
        assert!(!o.fresh(get), "a snapshot older than one applied");
        let (newer, next) = o.written();
        assert!(newer, "cache_ttl stays sending: a newer value is queued");
        // In the order of the latest picks: api_retries, then cache_ttl's last value.
        let next = next.unwrap();
        assert_eq!((next.1, next.2.as_deref()), ("api_retries", Some("4")));
        assert!(o.fresh(next.0));
        let (newer, next) = o.written();
        assert!(!newer);
        let next = next.unwrap();
        assert_eq!((next.1, next.2.as_deref()), ("cache_ttl", Some("hybrid")), "never the older 5m");
        assert!(o.fresh(next.0));
        assert!(o.written() == (false, None) && !o.writing());
        // Another dash: what was on its way is stale, the queue goes.
        let w = o.write("bash_timeout", Some("9".into())).unwrap();
        assert!(o.write("bash_timeout", Some("10".into())).is_none());
        o.reset();
        assert!(!o.fresh(w.0));
        assert_eq!(o.written(), (false, None));
    }

    #[test]
    fn view_keys_parse() {
        for mac in [false, true] {
            let (a, b, c) = view_keys(mac);
            for k in [Some(a), Some(b), c].into_iter().flatten() {
                gpui_kit::Keystroke::parse(k).unwrap_or_else(|e| panic!("{k}: {e:?}"));
            }
        }
    }
}
