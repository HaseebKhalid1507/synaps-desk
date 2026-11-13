//! The main window (the approved "hybrid" layout): a top bar with what
//! needs you, the sessions rail, the session in the middle (header, tool-call
//! timeline, authorization card, composer) and its numbers on the right.
//! Every colour comes from the accent palette ([`crate::theme`]); every
//! number from the daemon ([`crate::stats`]).

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::text::TextView;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use synaps_client::transcript::{Item, ItemKind, ToolStatus};
use synaps_client::{AttachMode, Client, CreateOptions, Cwd, Event, RequestError};

use crate::bench::BenchState;
use crate::files::{self, LineKind, Preview};
use crate::fonts::{self, label};
use crate::model::{CONFIRM_NO, CONFIRM_YES, ConnState, Effect, Filter, Model, RenameRequest, SAVED_LIMIT};
use crate::named::ResumeRequest;
use crate::theme::{self, Palette};

/// Per-row strings derived from a transcript item, rebuilt only when the
/// item's `rev` changes (so a frame never re-allocates unchanged rows).
pub(crate) struct RowCache {
    id: u64,
    rev: u64,
    text: SharedString,
    /// Tool rows: the tracked tool name, the one-line argument, the result's
    /// first lines and the diff of a file tool.
    name: SharedString,
    arg: SharedString,
    result: SharedString,
    result_more: usize,
    preview: Option<Arc<Preview>>,
}

/// Result lines an open tool row shows.
const RESULT_LINES: usize = 12;

pub struct AppView {
    pub client: Option<Client>,
    /// Connection, sessions, attachment and transcript. Events are applied
    /// to it as they arrive (the pump), never queued for `render`.
    pub model: Model,
    pub(crate) rows: Vec<RowCache>,
    pub list: ListState,
    /// Rows whose open/closed state the user flipped from its default.
    toggled: HashSet<u64>,
    composer: Entity<TextareaState>,
    /// "Open by name…" (rail) and "Name this session…" (header) inputs.
    pub(crate) open_input: Entity<InputState>,
    name_input: Entity<InputState>,
    /// The accent picker's custom colour and a secret prompt's answer.
    pub(crate) hex_input: Entity<InputState>,
    secret_input: Entity<InputState>,
    pub(crate) open_shown: bool,
    pub(crate) named_collapsed: bool,
    pub(crate) filter: Filter,
    pub picker_open: bool,
    pub(crate) picker_note: Option<SharedString>,
    /// Prompts answered here, not yet resolved by the daemon.
    answered: HashSet<u64>,
    /// Refused input to put back in the composer (needs a `Window`: done
    /// at the next frame).
    restore: Option<String>,
    pub bench: Option<BenchState>,
    /// Diagnostics: render rows as plain text instead of markdown.
    pub plain_rows: bool,
    /// The synthetic transport feeds this window (`--offline`).
    pub(crate) synthetic: bool,
    /// `--session ID`: attach to it once connected.
    pub start_session: Option<String>,
    _tasks: Vec<Task<()>>,
    _subs: Vec<Subscription>,
}

/// A text run in the display face: caps, tracked, semibold.
pub(crate) fn display(text: impl Into<SharedString>, size: f32, color: Hsla) -> Div {
    div()
        .font_family(fonts::DISPLAY)
        .font_weight(FontWeight::SEMIBOLD)
        .text_size(px(size))
        .text_color(color)
        .whitespace_nowrap()
        .child(text.into())
}

pub(crate) fn mono(text: impl Into<SharedString>, size: f32, color: Hsla) -> Div {
    div().font_family(fonts::MONO).text_size(px(size)).text_color(color).whitespace_nowrap().child(text.into())
}

/// A section header: display caps plus the Japanese sub-label.
pub(crate) fn section(title: &'static str, jp: &'static str, p: &Palette) -> Div {
    h_flex()
        .items_baseline()
        .gap(px(10.))
        .child(display(title, 11.5, p.ink))
        .child(div().font_family(fonts::JP).text_size(px(10.)).text_color(p.ink4).child(jp))
}

impl AppView {
    pub fn new(
        client: Option<(Client, async_channel::Receiver<Event>)>,
        synthetic: Option<Vec<Event>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 6)
                .submit_on_enter(true)
                .placeholder("Message, or steer a running turn  (Enter sends, Shift+Enter: new line)")
        });
        let open_input = cx.new(|cx| InputState::new(window, cx).placeholder("session name"));
        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("name: a-z, 0-9, '-'"));
        let hex_input = cx.new(|cx| InputState::new(window, cx).placeholder("#RRGGBB"));
        let secret_input = cx.new(|cx| InputState::new(window, cx).masked(true).placeholder("answer (hidden)"));
        let subs = vec![
            cx.subscribe_in(&composer, window, |this: &mut Self, input, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { shift: false, .. } = ev {
                    let text = input.read(cx).value().trim().to_string();
                    if !text.is_empty() && this.submit(text) {
                        input.update(cx, |s, cx| s.set_value("", window, cx));
                    }
                }
            }),
            // The typed name stays until the outcome is known: cleared on
            // `Attached` (`named.opened`, in `render`), kept on an error.
            cx.subscribe_in(&open_input, window, |this: &mut Self, input, ev: &InputEvent, _window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    let typed = input.read(cx).value().to_string();
                    for fx in this.model.open_by_name(&typed) {
                        this.effect(fx);
                    }
                    cx.notify();
                }
            }),
            cx.subscribe_in(&name_input, window, |this: &mut Self, input, ev: &InputEvent, _window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    let typed = input.read(cx).value().to_string();
                    let req = this.model.rename_request(&typed);
                    this.rename(req);
                    cx.notify();
                }
            }),
            cx.subscribe_in(&hex_input, window, |this: &mut Self, _input, ev: &InputEvent, _window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.apply_custom_accent(cx);
                }
            }),
            cx.subscribe_in(&secret_input, window, |this: &mut Self, _input, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.answer_secret(true, window, cx);
                }
            }),
        ];
        let inputs = Inputs { composer, open_input, name_input, hex_input, secret_input };
        let mut v = match (client, synthetic) {
            (Some((c, rx)), _) => {
                let mut v = Self::blank(inputs, Some(c), ConnState::Connecting, subs);
                v.pump(rx, cx);
                v
            }
            (None, Some(events)) => {
                let mut v = Self::blank(inputs, None, ConnState::Off("offline · synthetic transport".into()), subs);
                v.synthetic = true;
                v.model.set_local_session("synthetic");
                let (tx, rx) = async_channel::unbounded();
                for e in events {
                    let _ = tx.try_send(e);
                }
                v.pump(rx, cx);
                v
            }
            (None, None) => Self::blank(inputs, None, ConnState::Off("offline".into()), subs),
        };
        v.filter = Filter::All;
        v
    }

    fn blank(i: Inputs, client: Option<Client>, conn: ConnState, subs: Vec<Subscription>) -> Self {
        let list = ListState::new(0, ListAlignment::Top, px(600.));
        list.set_follow_mode(FollowMode::Tail);
        Self {
            client,
            model: Model::new(conn),
            rows: Vec::new(),
            list,
            toggled: HashSet::new(),
            composer: i.composer,
            open_input: i.open_input,
            name_input: i.name_input,
            hex_input: i.hex_input,
            secret_input: i.secret_input,
            open_shown: false,
            named_collapsed: false,
            filter: Filter::All,
            picker_open: false,
            picker_note: None,
            answered: HashSet::new(),
            restore: None,
            bench: None,
            plain_rows: false,
            synthetic: false,
            start_session: None,
            _tasks: Vec::new(),
            _subs: subs,
        }
    }

    /// Drain an event channel ([`crate::model::pump`]) and apply each batch
    /// to the model at once, whether or not the window renders (minimized,
    /// hidden, occluded): the queue can never grow behind a window that
    /// gets no frames. Only the row rebuild and remeasure wait for the next
    /// frame (`sync_list`), and the transcript coalesces those into one
    /// splice.
    pub fn pump(&mut self, rx: async_channel::Receiver<Event>, cx: &mut Context<Self>) {
        let task = cx.spawn(async move |this, cx| {
            crate::model::pump(rx, |batch| {
                let now = Instant::now();
                this.update(cx, |this, cx| {
                    if let Some(b) = this.bench.as_mut() {
                        b.on_received(&batch, now);
                    }
                    for ev in batch {
                        for fx in this.model.apply(ev) {
                            this.effect(fx);
                        }
                    }
                    let pending = this.model.transcript.pending_prompts();
                    this.answered.retain(|id| pending.iter().any(|p| p.id == *id));
                    if matches!(this.model.conn, ConnState::Connected(_))
                        && let Some(id) = this.start_session.take()
                    {
                        this.attach(id);
                    }
                    cx.notify();
                })
                .is_ok()
            })
            .await;
        });
        self._tasks.push(task);
    }

    pub(crate) fn effect(&mut self, fx: Effect) {
        match fx {
            Effect::Restore(text) => {
                self.restore = Some(match self.restore.take() {
                    Some(prev) => format!("{prev}\n{text}"),
                    None => text,
                })
            }
            Effect::ListSessions => {
                if let Some(c) = &self.client {
                    let _ = c.list_sessions();
                }
            }
            Effect::Resync => {
                if let Some(c) = &self.client {
                    let _ = c.resync();
                }
            }
            Effect::ListSaved => {
                let sent = match &self.client {
                    Some(c) => c.saved_sessions(true, SAVED_LIMIT).map_err(|e| e.to_string()),
                    None => Err("offline".into()),
                };
                if let Err(e) = sent {
                    for fx in self.model.saved_failed(&e) {
                        self.effect(fx);
                    }
                }
            }
            Effect::Attach { session_id, fallback, open } => {
                if let Some(seq) = self.client.as_ref().and_then(|c| c.attach(session_id, AttachMode::Mirror).ok()) {
                    self.model.attaching(seq, fallback, open);
                    self.list.set_follow_mode(FollowMode::Tail);
                } else if self.synthetic {
                    self.model.last_error = Some("offline: the synthetic transport holds one session".into());
                }
            }
            Effect::Resume { req, open } => self.resume(req, open),
            Effect::Detach => {
                if let Some(c) = &self.client {
                    let _ = c.detach();
                }
            }
        }
    }

    /// Resume a saved session: its folder as is, `~` if the daemon says it
    /// is gone there.
    fn resume(&mut self, req: ResumeRequest, open: bool) {
        let Some(c) = &self.client else {
            self.model.named.error = Some("offline".into());
            return;
        };
        match c.resume(req.target.clone(), req.options()) {
            Ok(seq) => {
                self.model.resuming(req, seq, open);
                self.list.set_follow_mode(FollowMode::Tail);
            }
            Err(e) => self.model.named.error = Some(e.to_string()),
        }
    }

    /// "Name this session…": send what the model checked.
    fn rename(&mut self, req: Option<RenameRequest>) {
        let Some(RenameRequest { session_id, name, expect }) = req else { return };
        let sent = match &self.client {
            Some(c) => c.rename(session_id, Some(&name), expect),
            None => Err(RequestError::NotConnected),
        };
        if let Err(e) = sent {
            self.model.rename_not_sent(e.to_string());
        }
    }

    pub(crate) fn attach(&mut self, session_id: String) {
        if self.model.is_active(&session_id) {
            return;
        }
        match self.client.as_ref().and_then(|c| c.attach(session_id, AttachMode::Mirror).ok()) {
            Some(seq) => {
                self.model.want(seq);
                self.list.set_follow_mode(FollowMode::Tail);
            }
            None if self.synthetic => self.model.last_error = Some("offline: the synthetic transport holds one session".into()),
            None => {}
        }
    }

    pub(crate) fn create(&mut self) {
        // Tools run in the user's home, not wherever the launcher started us.
        if let Some(seq) = self.client.as_ref().and_then(|c| c.create(CreateOptions::new().cwd(Cwd::Home)).ok()) {
            self.model.want(seq);
            self.list.set_follow_mode(FollowMode::Tail);
        }
    }

    /// Send the composer's text. `false` keeps it in the composer.
    fn submit(&mut self, text: String) -> bool {
        let Some(c) = &self.client else {
            self.model.last_error = Some("offline".into());
            return false;
        };
        if !self.model.can_send() {
            self.model.last_error = Some(if self.model.active.is_some() {
                "this session is read-only here (observing or reconnecting)".into()
            } else {
                "not attached to a session".into()
            });
            return false;
        }
        let sent = if self.model.transcript.busy() { c.steer(text.clone()) } else { c.submit(text.clone()) };
        if sent.is_err() {
            self.model.last_error = Some("the connection thread has stopped".into());
            return false;
        }
        self.model.sent(text);
        true
    }

    /// A daemon is connected and this attachment may drive it (the
    /// synthetic transport never can).
    fn drives(&self) -> bool {
        self.client.is_some() && self.model.can_send()
    }

    /// Answer a pending prompt (`Client::answer`). A confirm takes `y` / `n`
    /// (the daemon's own spelling); a secret its text, or nothing to cancel.
    fn answer(&mut self, prompt_id: u64, value: Option<String>) {
        let sent = match &self.client {
            Some(c) if self.model.can_send() => c.answer(prompt_id, value).map_err(|_| "the connection thread has stopped".to_string()),
            Some(_) => Err("this session is read-only here (observing or reconnecting)".into()),
            None => Err("offline: there is no daemon to answer".into()),
        };
        match sent {
            Ok(()) => {
                self.answered.insert(prompt_id);
            }
            Err(e) => self.model.last_error = Some(e),
        }
    }

    fn answer_secret(&mut self, submit: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.model.transcript.pending_prompts().first().map(|p| p.id) else { return };
        let value = submit.then(|| self.secret_input.read(cx).value().to_string());
        self.answer(id, value);
        self.secret_input.update(cx, |s, cx| s.set_value("", window, cx));
        cx.notify();
    }

    pub(crate) fn set_accent(&mut self, hex: u32, cx: &mut Context<Self>) {
        theme::apply(hex, cx);
        self.picker_note = Some(match theme::settings_path() {
            Some(path) => match theme::save_accent(&path, hex) {
                Ok(()) => format!("saved to {}", path.display()).into(),
                Err(e) => format!("not saved: {e}").into(),
            },
            None => "not saved: no config folder".into(),
        });
        cx.notify();
    }

    pub(crate) fn apply_custom_accent(&mut self, cx: &mut Context<Self>) {
        let typed = self.hex_input.read(cx).value().to_string();
        match theme::parse_hex(&typed) {
            Some(hex) => self.set_accent(hex, cx),
            None => {
                self.picker_note = Some("not a colour: use #RRGGBB".into());
                cx.notify();
            }
        }
    }

    /// Push transcript changes into the virtual list: only the changed tail
    /// is spliced (and so re-measured).
    pub fn sync_list(&mut self) {
        let Some(sp) = self.model.transcript.take_splice() else { return };
        self.rows.truncate(sp.start);
        for item in &self.model.transcript.items()[sp.start..] {
            self.rows.push(row_cache(item, &self.model.stats.files));
        }
        debug_assert!(self.rows.iter().zip(self.model.transcript.items()).all(|(r, i)| r.id == i.id && r.rev == i.rev));
        self.list.splice(sp.start..sp.old_len, sp.new_len - sp.start);
    }

    /// Open by default: a file tool's diff when it is short.
    fn is_open(&self, item: &Item, row: &RowCache) -> bool {
        let default = item.kind == ItemKind::Tool && row.preview.as_ref().is_some_and(|p| p.lines.len() <= 24 && !p.lines.is_empty());
        default ^ self.toggled.contains(&item.id)
    }

    fn toggle(&mut self, id: u64) {
        if !self.toggled.remove(&id) {
            self.toggled.insert(id);
        }
        if let Some(ix) = self.model.transcript.items().iter().position(|i| i.id == id) {
            self.list.remeasure_items(ix..ix + 1);
        }
    }

    fn render_row(&self, ix: usize, cx: &Context<Self>) -> AnyElement {
        let (Some(item), Some(row)) = (self.model.transcript.items().get(ix), self.rows.get(ix)) else {
            return div().into_any_element();
        };
        let p = theme::palette(cx);
        let id = item.id;
        let body: AnyElement = match item.kind {
            ItemKind::User => div()
                .mt(px(14.))
                .pl(px(16.))
                .py(px(1.))
                .border_l_2()
                .border_color(p.ink)
                .text_size(px(14.))
                .text_color(p.ink)
                .child(TextView::markdown(("md", id), row.text.clone()))
                .into_any_element(),
            ItemKind::Assistant => v_flex()
                .mt(px(10.))
                .gap(px(6.))
                .child(display(label::AGENT, 10., p.ink3))
                .child(if self.plain_rows {
                    div().text_size(px(14.)).child(row.text.clone()).into_any_element()
                } else {
                    div().text_size(px(14.)).child(TextView::markdown(("md", id), row.text.clone())).into_any_element()
                })
                .into_any_element(),
            ItemKind::Thinking => {
                let open = self.toggled.contains(&id);
                v_flex()
                    .child(self.row_header(id, open, label::THINK, p.ink3, row.arg.clone(), None, &p, cx))
                    .when(open, |d| {
                        d.child(
                            div()
                                .ml(px(24.))
                                .mb(px(8.))
                                .pl(px(16.))
                                .border_l_1()
                                .border_color(p.line3)
                                .text_size(px(13.))
                                .text_color(p.ink3)
                                .child(row.text.clone()),
                        )
                    })
                    .into_any_element()
            }
            ItemKind::Tool => {
                let status = item.tool.as_ref().and_then(|t| t.status);
                let open = self.is_open(item, row);
                let has_body = row.preview.is_some() || !row.result.is_empty();
                let state = self.tool_state(status, &p);
                v_flex()
                    .child(self.row_header(id, open && has_body, row.name.clone(), p.ink2, row.arg.clone(), Some(state), &p, cx))
                    .when(open && has_body, |d| match &row.preview {
                        Some(pv) => d.child(diff_block(pv, &p)),
                        None => d.child(result_block(&row.result, row.result_more, status == Some(ToolStatus::Failed), &p)),
                    })
                    .into_any_element()
            }
            ItemKind::Notice => h_flex()
                .my(px(6.))
                .gap(px(12.))
                .child(div().w(px(18.)).h(px(1.)).bg(p.line2))
                .child(mono(row.text.clone(), 10.5, p.ink4).overflow_hidden().text_ellipsis())
                .into_any_element(),
            ItemKind::Error => mono(row.text.clone(), 11.5, p.hi).my(px(6.)).whitespace_normal().into_any_element(),
            _ => div().text_size(px(13.)).child(row.text.clone()).into_any_element(),
        };
        div().w_full().pl(px(36.)).pr(px(40.)).py(px(1.)).child(div().w_full().max_w(px(836.)).child(body)).into_any_element()
    }

    /// A tool's status cell: a check, a cross, or (only while a turn runs
    /// and is not waiting on the user) a pulsing dot.
    fn tool_state(&self, status: Option<ToolStatus>, p: &Palette) -> AnyElement {
        match status {
            Some(ToolStatus::Done) => mono("✓", 11., p.ink4).into_any_element(),
            Some(ToolStatus::Failed) => mono("× failed", 10.5, p.hi).into_any_element(),
            Some(ToolStatus::Cancelled) => mono("cancelled", 10.5, p.ink4).into_any_element(),
            Some(ToolStatus::Queued) => mono("queued", 10.5, p.ink4).into_any_element(),
            _ => {
                let live = self.model.transcript.busy() && self.model.transcript.pending_prompts().is_empty();
                let dot = div().size(px(6.)).bg(p.ink2);
                h_flex()
                    .gap(px(6.))
                    .child(if live {
                        dot.with_animation(
                            "tool-running",
                            Animation::new(Duration::from_millis(900)).repeat().with_easing(pulsating_between(0.25, 1.0)),
                            |d, t| d.opacity(t),
                        )
                        .into_any_element()
                    } else {
                        dot.into_any_element()
                    })
                    .child(mono(if live { "running" } else { "waiting" }, 10.5, p.ink2))
                    .into_any_element()
            }
        }
    }

    /// A collapsible row's header: sign, name, argument, state.
    #[allow(clippy::too_many_arguments)]
    fn row_header(
        &self,
        id: u64,
        open: bool,
        name: impl Into<SharedString>,
        name_color: Hsla,
        arg: SharedString,
        state: Option<AnyElement>,
        p: &Palette,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .id(("row", id))
            .h(px(30.))
            .px(px(4.))
            .gap(px(12.))
            .items_center()
            .cursor_pointer()
            .hover(|d| d.bg(p.bg2))
            .child(mono(if open { "−" } else { "+" }, 11., p.ink4).w(px(10.)).flex_none())
            .child(display(name, 10.5, name_color).w(px(76.)).flex_none().overflow_hidden())
            .child(mono(arg, 11.5, p.ink3).flex_1().min_w_0().overflow_hidden().text_ellipsis())
            .children(state)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.toggle(id);
                cx.notify();
            }))
    }

    /// The center column's header: title, model · effort, folder.
    fn render_header(&self, p: &Palette, cx: &Context<Self>) -> impl IntoElement {
        let attached = self.model.active.is_some() && (self.client.is_some() || self.synthetic);
        let title: SharedString = self.model.transcript.title().or(self.model.active_name()).unwrap_or("No session").to_string().into();
        let view = self.model.stats.view.as_ref();
        let model = view.map(|v| v.model.as_str()).filter(|m| !m.is_empty()).or(self.model.active_meta().map(|s| s.model.as_str()));
        let meta: Vec<String> = [model.map(crate::named::short_model), view.map(|v| v.effort.clone()).filter(|e| !e.is_empty())]
            .into_iter()
            .flatten()
            .collect();
        let cwd = self.model.stats.cwd.as_deref().or(self.model.active_meta().and_then(|s| s.cwd.as_deref())).map(home_short);
        let busy = self.model.transcript.busy() && self.drives();
        v_flex()
            .flex_none()
            .pt(px(22.))
            .pb(px(18.))
            .pl(px(36.))
            .pr(px(30.))
            .border_b_1()
            .border_color(p.line)
            .child(
                h_flex()
                    .items_baseline()
                    .gap(px(20.))
                    .child(display(title, 23., p.ink).min_w_0().overflow_hidden().text_ellipsis())
                    .when_some(self.model.active_name(), |d, n| d.child(mono(n.to_string(), 10.5, p.ink3)))
                    .child(div().flex_1())
                    .when(attached, |d| {
                        d.child(tbtn("name-session", label::NAME, p.ink3, p.ink).on_click(cx.listener(|this, _, window, cx| {
                            this.model.toggle_naming();
                            if this.model.named.naming {
                                let current = this.model.active_name().unwrap_or_default().to_string();
                                this.name_input.update(cx, |s, cx| {
                                    s.set_value(current, window, cx);
                                    s.focus(window, cx);
                                });
                            }
                            cx.notify();
                        })))
                    })
                    .when(busy, |d| {
                        d.child(tbtn("halt", label::HALT, p.ink3, p.hi).on_click(cx.listener(|this, _, _, _| {
                            if let Some(c) = &this.client {
                                let _ = c.abort();
                            }
                        })))
                    }),
            )
            .when(!meta.is_empty() || cwd.is_some(), |d| {
                d.child(
                    h_flex()
                        .mt(px(8.))
                        .gap(px(24.))
                        .when(!meta.is_empty(), |d| d.child(mono(meta.join("  ·  "), 10.5, p.ink3)))
                        .when_some(cwd, |d, c| d.child(mono(c, 10.5, p.ink4).overflow_hidden().text_ellipsis())),
                )
            })
    }

    /// The "Name this session…" row under the header.
    fn render_naming(&self, p: &Palette, cx: &Context<Self>) -> impl IntoElement {
        let pending = self.model.named.rename.is_some();
        let mut row = v_flex().pl(px(36.)).pr(px(30.)).py(px(10.)).gap(px(6.)).border_b_1().border_color(p.line).child(
            h_flex()
                .gap(px(14.))
                .items_center()
                .child(display(label::NAME, 10., p.ink4))
                .child(div().flex_1().max_w(px(320.)).child(Input::new(&self.name_input)))
                .child(tbtn("name-save", if pending { "S A V I N G" } else { label::SAVE }, p.ink2, p.ink).on_click(cx.listener(
                    |this, _, _, cx| {
                        let typed = this.name_input.read(cx).value().to_string();
                        let req = this.model.rename_request(&typed);
                        this.rename(req);
                        cx.notify();
                    },
                )))
                .child(tbtn("name-cancel", label::CANCEL, p.ink4, p.ink2).on_click(cx.listener(|this, _, _, cx| {
                    this.model.named.naming = false;
                    cx.notify();
                }))),
        );
        if let Some(e) = &self.model.named.rename_error {
            row = row.child(div().text_size(px(12.)).text_color(p.hi).child(e.clone()));
        }
        // Only for the session it was made for (a switch drops it).
        if let Some(r) = self.model.named.replace.as_ref().filter(|r| self.model.is_active(&r.session_id)) {
            row = row.child(
                h_flex()
                    .gap(px(12.))
                    .child(div().text_size(px(12.)).text_color(p.ink2).child(format!("Replace '{}' with '{}'?", r.current, r.asked)))
                    .child(tbtn("name-replace", label::REPLACE, p.hi, p.ink).on_click(cx.listener(|this, _, _, cx| {
                        let req = this.model.replace_request();
                        this.rename(req);
                        cx.notify();
                    }))),
            );
        }
        row
    }

    /// The authorization card: the first pending prompt, answered through
    /// `Client::answer`.
    fn render_prompt(&self, p: &Palette, cx: &Context<Self>) -> Option<impl IntoElement> {
        let prompts = self.model.transcript.pending_prompts();
        let pr = prompts.first()?;
        let secret = pr.is_secret();
        let sent = self.answered.contains(&pr.id);
        let can = self.drives() && !sent;
        let id = pr.id;
        let (head, kind) = if secret { (label::SECRET, "S E C R E T") } else { (label::AUTHORIZATION, "C O N F I R M") };
        let lead: SharedString = pr.title.clone().into();
        let text: SharedString = pr.prompt.lines().filter(|l| !l.trim().is_empty()).take(12).collect::<Vec<_>>().join("\n").into();
        let actions = if secret {
            h_flex()
                .gap(px(22.))
                .items_center()
                .child(div().w(px(260.)).child(Input::new(&self.secret_input).disabled(!can)))
                .child(
                    gbtn("p-submit", label::SUBMIT, can, p)
                        .on_click(cx.listener(|this, _, window, cx| this.answer_secret(true, window, cx))),
                )
                .child(
                    tbtn("p-cancel", label::CANCEL, if can { p.hi } else { p.ink4 }, p.ink)
                        .on_click(cx.listener(|this, _, window, cx| this.answer_secret(false, window, cx))),
                )
        } else {
            h_flex()
                .gap(px(22.))
                .items_center()
                .child(gbtn("p-yes", label::AUTHORIZE, can, p).on_click(cx.listener(move |this, _, _, cx| {
                    if this.drives() {
                        this.answer(id, Some(CONFIRM_YES.into()));
                    }
                    cx.notify();
                })))
                .child(tbtn("p-no", label::DENY, if can { p.hi } else { p.ink4 }, p.ink).on_click(cx.listener(move |this, _, _, cx| {
                    if this.drives() {
                        this.answer(id, Some(CONFIRM_NO.into()));
                    }
                    cx.notify();
                })))
        };
        let footer: Option<SharedString> = if sent {
            Some("answer sent, waiting for the daemon".into())
        } else if !self.drives() {
            Some(if self.client.is_none() { "offline: nothing to answer" } else { "read-only here" }.into())
        } else if prompts.len() > 1 {
            Some(format!("+{} more waiting", prompts.len() - 1).into())
        } else {
            None
        };
        Some(
            div().pl(px(36.)).pr(px(30.)).pt(px(8.)).pb(px(4.)).child(
                v_flex()
                    .max_w(px(720.))
                    .px(px(22.))
                    .py(px(18.))
                    .border_1()
                    .border_color(p.mid)
                    .bg(p.accent_bg)
                    .child(
                        h_flex()
                            .items_center()
                            .gap(px(12.))
                            .child(div().size(px(7.)).bg(p.accent))
                            .child(display(head, 12., p.hi))
                            .child(div().flex_1())
                            .child(display(kind, 9.5, p.ink4)),
                    )
                    .when(!lead.is_empty(), |d| d.child(div().mt(px(10.)).text_size(px(13.)).text_color(p.ink2).child(lead)))
                    .when(!text.is_empty(), |d| {
                        d.child(
                            div()
                                .mt(px(10.))
                                .px(px(14.))
                                .py(px(11.))
                                .bg(p.well)
                                .font_family(fonts::MONO)
                                .text_size(px(12.5))
                                .text_color(p.ink)
                                .child(text),
                        )
                    })
                    .child(
                        h_flex()
                            .mt(px(16.))
                            .items_center()
                            .child(actions)
                            .child(div().flex_1())
                            .when_some(footer, |d, f| d.child(mono(f, 10., p.ink4))),
                    ),
            ),
        )
    }

    fn render_composer(&self, p: &Palette, cx: &Context<Self>) -> impl IntoElement {
        let busy = self.model.transcript.busy();
        let can_send = self.drives();
        div().flex_none().pl(px(36.)).pr(px(30.)).pt(px(6.)).pb(px(22.)).child(
            v_flex()
                .px(px(16.))
                .pt(px(12.))
                .pb(px(11.))
                .border_1()
                .border_color(p.line3)
                .bg(p.bg1)
                .child(Textarea::new(&self.composer).appearance(false).disabled(!can_send))
                .child(
                    h_flex()
                        .mt(px(10.))
                        .items_center()
                        .gap(px(20.))
                        .child(mono(
                            if self.client.is_none() {
                                "offline: no daemon to send to"
                            } else if !can_send {
                                "not attached"
                            } else if busy {
                                "steer: applied at the next turn boundary"
                            } else {
                                "send: starts a turn"
                            },
                            10.5,
                            p.ink4,
                        ))
                        .child(div().flex_1())
                        .child(gbtn("send", if busy { label::STEER } else { label::SEND }, can_send, p).on_click(cx.listener(
                            |this, _, window, cx| {
                                let text = this.composer.read(cx).value().trim().to_string();
                                if !text.is_empty() && this.submit(text) {
                                    this.composer.update(cx, |s, cx| s.set_value("", window, cx));
                                }
                                cx.notify();
                            },
                        ))),
                ),
        )
    }
}

/// A text button: tracked caps, `color`, `hover` on hover.
pub(crate) fn tbtn(id: &'static str, text: &'static str, color: Hsla, hover: Hsla) -> Stateful<Div> {
    display(text, 10.5, color).id(id).cursor_pointer().hover(move |d| d.text_color(hover))
}

/// The filled button (ink on the background), dimmed when it cannot act.
pub(crate) fn gbtn(id: &'static str, text: &'static str, enabled: bool, p: &Palette) -> Stateful<Div> {
    let (bg, fg) = if enabled { (p.ink, p.bg) } else { (p.line3, p.ink4) };
    let hover = if enabled { p.hi } else { bg };
    div()
        .id(id)
        .h(px(28.))
        .px(px(14.))
        .flex()
        .items_center()
        .bg(bg)
        .when(enabled, |d| d.cursor_pointer())
        .hover(move |d| d.bg(hover))
        .child(display(text, 11., fg))
}

fn diff_block(pv: &Preview, p: &Palette) -> impl IntoElement {
    let mut col =
        v_flex().ml(px(24.)).mb(px(10.)).py(px(2.)).border_l_1().border_color(p.line3).font_family(fonts::MONO).text_size(px(11.5));
    for l in &pv.lines {
        let (sign, fg, bg) = match l.kind {
            LineKind::Add => ("+", p.ink, Some(p.add)),
            LineKind::Del => ("−", p.soft, Some(p.tint)),
            LineKind::Context => (" ", p.ink2, None),
            LineKind::Note => (" ", p.ink4, None),
        };
        col = col.child(
            h_flex()
                .px(px(14.))
                .h(px(18.))
                .when_some(bg, |d, bg| d.bg(bg))
                .child(div().w(px(14.)).flex_none().text_color(if l.kind == LineKind::Del { p.hi } else { p.ink4 }).child(sign))
                .child(
                    div().text_color(fg).whitespace_nowrap().overflow_hidden().text_ellipsis().child(SharedString::from(l.text.clone())),
                ),
        );
    }
    if pv.more > 0 {
        col = col.child(div().px(px(14.)).text_color(p.ink4).child(format!("… {} more lines", pv.more)));
    }
    col
}

fn result_block(result: &SharedString, more: usize, failed: bool, p: &Palette) -> impl IntoElement {
    v_flex()
        .ml(px(24.))
        .mb(px(10.))
        .px(px(14.))
        .py(px(2.))
        .border_l_1()
        .border_color(p.line3)
        .font_family(fonts::MONO)
        .text_size(px(11.5))
        .text_color(if failed { p.hi } else { p.ink2 })
        .child(result.clone())
        .when(more > 0, |d| d.child(div().text_color(p.ink4).child(format!("… {more} more lines"))))
}

fn row_cache(item: &Item, ledger: &files::Ledger) -> RowCache {
    let mut r = RowCache {
        id: item.id,
        rev: item.rev,
        text: SharedString::default(),
        name: SharedString::default(),
        arg: SharedString::default(),
        result: SharedString::default(),
        result_more: 0,
        preview: None,
    };
    match item.kind {
        ItemKind::Tool => {
            let Some(t) = item.tool.as_ref() else { return r };
            r.name = tracked(&t.name).into();
            r.preview = files::is_file_tool(&t.name).then(|| ledger.preview(&t.tool_id)).flatten();
            r.arg = match &r.preview {
                Some(pv) if !pv.title.is_empty() => pv.title.clone().into(),
                _ => one_line(&t.input, 200).into(),
            };
            let lines: Vec<&str> = t.result.lines().collect();
            r.result_more = lines.len().saturating_sub(RESULT_LINES);
            r.result = lines.iter().take(RESULT_LINES).copied().collect::<Vec<_>>().join("\n").into();
        }
        ItemKind::Thinking => {
            r.text = item.text.clone().into();
            r.arg = one_line(&item.text, 200).into();
        }
        _ => r.text = item.text.clone().into(),
    }
    r
}

/// `multi_edit` → `M U L T I _ E D I T`.
fn tracked(name: &str) -> String {
    let mut out = String::with_capacity(name.len() * 2);
    for (i, c) in name.chars().flat_map(char::to_uppercase).enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

/// `/home/me/x` → `~/x` when it is this machine's home.
pub(crate) fn home_short(path: &str) -> String {
    let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_default();
    match path.strip_prefix(home.as_str()) {
        Some(rest) if !home.is_empty() && (rest.is_empty() || rest.starts_with(['/', '\\'])) => format!("~{rest}"),
        _ => path.to_string(),
    }
}

pub(crate) fn one_line(s: &str, max: usize) -> String {
    let line = s.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    let more = s.lines().filter(|l| !l.trim().is_empty()).count() > 1;
    let mut out: String = line.chars().take(max).collect();
    if more || line.chars().count() > max {
        out.push_str(" …");
    }
    out
}

struct Inputs {
    composer: Entity<TextareaState>,
    open_input: Entity<InputState>,
    name_input: Entity<InputState>,
    hex_input: Entity<InputState>,
    secret_input: Entity<InputState>,
}

impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::diag::mark("first_render");
        if let Some(text) = self.restore.take() {
            // Refused input goes back where it was typed, ahead of anything
            // typed since.
            self.composer.update(cx, |s, cx| {
                let now = s.value().trim().to_string();
                let text = if now.is_empty() { text } else { format!("{text}\n{now}") };
                s.set_value(text, window, cx);
            });
        }
        if std::mem::take(&mut self.model.named.opened) {
            self.open_input.update(cx, |s, cx| s.set_value("", window, cx));
            self.open_shown = false;
        }
        self.sync_list();
        if let Some(b) = self.bench.as_mut() {
            b.on_render(&self.list, window);
        }
        let p = theme::palette(cx);
        // Narrow windows drop the numbers, then the rail, before the session.
        let width = window.viewport_size().width;
        let (show_rail, show_right) = (width >= px(760.), width >= px(1100.));
        let list = list(self.list.clone(), cx.processor(|this, ix, _window, cx| this.render_row(ix, cx))).size_full();
        let main = v_flex()
            .flex_1()
            .h_full()
            .min_w_0()
            .child(self.render_header(&p, cx))
            .when(self.model.named.naming && self.model.active.is_some(), |d| d.child(self.render_naming(&p, cx)))
            .child(div().flex_1().min_h_0().pt(px(14.)).child(list))
            .children(self.render_prompt(&p, cx))
            .when_some(self.model.last_error.clone(), |d, e| {
                d.child(mono(e, 11., p.hi).pl(px(36.)).pr(px(30.)).pt(px(6.)).whitespace_normal())
            })
            .child(self.render_composer(&p, cx));
        h_flex()
            .relative()
            .size_full()
            .bg(p.bg)
            .text_color(p.ink)
            .font_family(fonts::SANS)
            .text_size(px(13.))
            .child(self.render_mark(&p))
            .child(
                v_flex().flex_1().h_full().min_w_0().child(self.render_top(&p, cx)).child(
                    h_flex()
                        .flex_1()
                        .min_h_0()
                        .when(show_rail, |d| d.child(self.render_rail(&p, cx)))
                        .child(main)
                        .when(show_right, |d| d.child(self.render_right(&p))),
                ),
            )
            .when(self.picker_open, |d| d.child(deferred(self.render_picker(&p, cx)).with_priority(1)))
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that brings in gpui's `test` attribute through `gpui_kit::*`.
    use super::{one_line, tracked};

    #[test]
    fn tracked_names_and_paths() {
        assert_eq!(tracked("multi_edit"), "M U L T I _ E D I T");
        assert_eq!(tracked("bash"), "B A S H");
        assert_eq!(tracked(""), "");
        assert_eq!(one_line("  \nfirst\nsecond", 80), "first …");
        assert_eq!(one_line("abcdef", 3), "abc …");
    }
}
