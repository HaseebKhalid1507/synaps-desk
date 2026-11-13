//! The main window: session rail, virtualized transcript, composer.

use std::collections::HashSet;
use std::time::Instant;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::text::TextView;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, h_flex, v_flex};
use gpui_kit::*;
use gpui_kit::prelude::FluentBuilder as _;
use synaps_client::transcript::{ItemKind, ToolStatus};
use synaps_client::{AttachMode, Client, CreateOptions, Cwd, Event, RequestError};

use crate::bench::BenchState;
use crate::model::{ConnState, Effect, Model, SAVED_LIMIT};
use crate::model::RenameRequest;
use crate::named::{Marker, ResumeRequest};

/// Per-row strings derived from a transcript item, rebuilt only when the
/// item's `rev` changes (so a frame never re-allocates unchanged rows).
struct RowCache {
    id: u64,
    rev: u64,
    text: SharedString,
    detail: SharedString,
}

pub struct AppView {
    pub client: Option<Client>,
    /// Connection, sessions, attachment and transcript. Events are applied
    /// to it as they arrive (the pump), never queued for `render`.
    pub model: Model,
    rows: Vec<RowCache>,
    pub list: ListState,
    expanded: HashSet<u64>,
    composer: Entity<TextareaState>,
    /// "Open by name…" (rail) and "Name this session…" (header) inputs.
    open_input: Entity<InputState>,
    name_input: Entity<InputState>,
    open_shown: bool,
    named_collapsed: bool,
    /// Refused input to put back in the composer (needs a `Window`: done
    /// at the next frame).
    restore: Option<String>,
    pub bench: Option<BenchState>,
    /// Diagnostics: render rows as plain text instead of markdown.
    pub plain_rows: bool,
    _tasks: Vec<Task<()>>,
    _subs: Vec<Subscription>,
}

impl AppView {
    pub fn new(
        client: Option<(Client, async_channel::Receiver<Event>)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 8)
                .submit_on_enter(true)
                .placeholder("Message Synaps  (Enter to send, Shift+Enter for a newline)")
        });
        let open_input = cx.new(|cx| InputState::new(window, cx).placeholder("session name"));
        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("name: a-z, 0-9, '-'"));
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
        ];
        let inputs = (composer, open_input, name_input);
        match client {
            Some((c, rx)) => {
                let mut v = Self::blank(inputs, Some(c), ConnState::Connecting, subs);
                v.pump(rx, cx);
                v
            }
            None => Self::blank(inputs, None, ConnState::Off("offline".into()), subs),
        }
    }

    fn blank(
        (composer, open_input, name_input): (Entity<TextareaState>, Entity<InputState>, Entity<InputState>),
        client: Option<Client>,
        conn: ConnState,
        subs: Vec<Subscription>,
    ) -> Self {
        let list = ListState::new(0, ListAlignment::Top, px(600.));
        list.set_follow_mode(FollowMode::Tail);
        Self {
            client,
            model: Model::new(conn),
            rows: Vec::new(),
            list,
            expanded: HashSet::new(),
            composer,
            open_input,
            name_input,
            open_shown: false,
            named_collapsed: false,
            restore: None,
            bench: None,
            plain_rows: false,
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
                    cx.notify();
                })
                .is_ok()
            })
            .await;
        });
        self._tasks.push(task);
    }

    fn effect(&mut self, fx: Effect) {
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

    fn attach(&mut self, session_id: String) {
        if let Some(seq) = self.client.as_ref().and_then(|c| c.attach(session_id, AttachMode::Mirror).ok()) {
            self.model.want(seq);
            self.list.set_follow_mode(FollowMode::Tail);
        }
    }

    fn create(&mut self) {
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

    /// Push transcript changes into the virtual list: only the changed tail
    /// is spliced (and so re-measured).
    pub fn sync_list(&mut self) {
        let Some(sp) = self.model.transcript.take_splice() else { return };
        self.rows.truncate(sp.start);
        for item in &self.model.transcript.items()[sp.start..] {
            let (text, detail) = match item.kind {
                ItemKind::Tool => {
                    let t = item.tool.as_ref();
                    let input = t.map(|t| one_line(&t.input, 160)).unwrap_or_default();
                    let result = t.map(|t| one_line(&t.result, 160)).unwrap_or_default();
                    (input, result)
                }
                _ => (item.text.clone(), String::new()),
            };
            self.rows.push(RowCache { id: item.id, rev: item.rev, text: text.into(), detail: detail.into() });
        }
        debug_assert!(self.rows.iter().zip(self.model.transcript.items()).all(|(r, i)| r.id == i.id && r.rev == i.rev));
        self.list.splice(sp.start..sp.old_len, sp.new_len - sp.start);
    }

    fn render_row(&self, ix: usize, cx: &Context<Self>) -> AnyElement {
        let (Some(item), Some(row)) = (self.model.transcript.items().get(ix), self.rows.get(ix)) else {
            return div().into_any_element();
        };
        let theme = cx.theme();
        let id = item.id;
        let body: AnyElement = match item.kind {
            ItemKind::User => div()
                .px_3()
                .py_2()
                .rounded_lg()
                .bg(theme.secondary)
                .child(TextView::markdown(("md", id), row.text.clone()))
                .into_any_element(),
            ItemKind::Assistant if self.plain_rows => div().child(row.text.clone()).into_any_element(),
            ItemKind::Assistant => TextView::markdown(("md", id), row.text.clone()).into_any_element(),
            ItemKind::Thinking => {
                let open = self.expanded.contains(&id);
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .id(("think", id))
                            .cursor_pointer()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(format!(
                                "{} Thinking{} · {} chars",
                                if open { "▾" } else { "▸" },
                                if item.streaming { "…" } else { "" },
                                row.text.len()
                            ))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.expanded.remove(&id) {
                                    this.expanded.insert(id);
                                }
                                if let Some(ix) = this.model.transcript.items().iter().position(|i| i.id == id) {
                                    this.list.remeasure_items(ix..ix + 1);
                                }
                                cx.notify();
                            })),
                    )
                    .when(open, |d| {
                        d.child(
                            div()
                                .pl_4()
                                .border_l_2()
                                .border_color(theme.border)
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child(row.text.clone()),
                        )
                    })
                    .into_any_element()
            }
            ItemKind::Tool => {
                let t = item.tool.as_ref();
                let status = t.and_then(|t| t.status);
                let (glyph, color, label) = match status {
                    Some(ToolStatus::Done) => ("✓", theme.success, "done"),
                    Some(ToolStatus::Failed) => ("✗", theme.danger, "failed"),
                    Some(ToolStatus::Queued) => ("◌", theme.muted_foreground, "queued"),
                    Some(ToolStatus::Streaming) => ("…", theme.muted_foreground, "preparing"),
                    Some(ToolStatus::Cancelled) => ("⊘", theme.muted_foreground, "cancelled"),
                    _ => ("●", theme.warning, "running"),
                };
                let name: SharedString = t.map(|t| t.name.clone()).unwrap_or_default().into();
                v_flex()
                    .px_3()
                    .py_1p5()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.muted)
                    .text_sm()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().text_color(color).child(glyph))
                            .child(div().font_weight(FontWeight::SEMIBOLD).child(name))
                            .child(
                                div()
                                    .flex_1()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .font_family(theme.mono_font_family.clone())
                                    .text_color(theme.muted_foreground)
                                    .child(row.text.clone()),
                            )
                            .child(div().text_color(color).child(label)),
                    )
                    .when(!row.detail.is_empty(), |d| {
                        d.child(
                            div()
                                .pl_6()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .font_family(theme.mono_font_family.clone())
                                .text_color(theme.muted_foreground)
                                .child(row.detail.clone()),
                        )
                    })
                    .into_any_element()
            }
            ItemKind::Notice => div().text_sm().text_color(theme.muted_foreground).child(row.text.clone()).into_any_element(),
            ItemKind::Error => div().text_sm().text_color(theme.danger).child(row.text.clone()).into_any_element(),
            _ => div().text_sm().child(row.text.clone()).into_any_element(),
        };
        div()
            .w_full()
            .flex()
            .justify_center()
            .px_6()
            .py_1p5()
            .child(div().w_full().max_w(px(860.)).child(body))
            .into_any_element()
    }

    fn render_named(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let rows = self.model.named_rows();
        let open = !self.named_collapsed;
        let header = h_flex()
            .px_2()
            .pt_1()
            .pb_0p5()
            .gap_1()
            .justify_between()
            .child(
                div()
                    .id("named-toggle")
                    .flex_1()
                    .cursor_pointer()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.muted_foreground)
                    .child(format!("{} NAMED · {}", if open { "▾" } else { "▸" }, rows.len()))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.named_collapsed = !this.named_collapsed;
                        cx.notify();
                    })),
            )
            .child(Button::new("open-by-name").ghost().compact().label("Open by name…").on_click(cx.listener(
                |this, _, window, cx| {
                    this.open_shown = !this.open_shown;
                    this.named_collapsed = false;
                    if this.open_shown {
                        this.model.open_shown();
                        this.open_input.update(cx, |s, cx| s.focus(window, cx));
                    }
                    cx.notify();
                },
            )));
        let mut section = v_flex().gap_0p5().child(header);
        if self.open_shown {
            section = section.child(div().px_2().pb_1().child(Input::new(&self.open_input)));
        }
        let n = &self.model.named;
        if let Some(e) = &n.error {
            section = section.child(div().px_2().text_xs().text_color(theme.danger).child(e.clone()));
        }
        let muted = [
            n.notice.clone(),
            n.saved_note.clone(),
            n.saved_truncated.then(|| format!("Showing the newest {SAVED_LIMIT} saved sessions.")),
        ];
        for note in muted.into_iter().flatten() {
            section = section.child(div().px_2().text_xs().text_color(theme.muted_foreground).child(note));
        }
        if !open {
            return section;
        }
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        for (i, row) in rows.iter().enumerate() {
            let active = matches!(&row.action, crate::named::RowAction::Attach(id) if self.model.active.as_ref().is_some_and(|a| &a.session_id == id));
            let color = match row.marker {
                Marker::Busy | Marker::Resuming => theme.warning,
                Marker::Awaiting => theme.danger,
                Marker::Idle | Marker::Pinned => theme.success,
                Marker::Parked | Marker::Saved => theme.muted_foreground,
            };
            let when = row.activity.as_deref().map(|t| crate::named::relative(t, now)).unwrap_or_default();
            let detail: SharedString =
                [row.marker.label(), row.model.as_str(), when.as_str()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ").into();
            let action = row.action.clone();
            section = section.child(
                h_flex()
                    .id(("named", i))
                    .px_2()
                    .py_1()
                    .gap_2()
                    .rounded_md()
                    .cursor_pointer()
                    .when(active, |d| d.bg(theme.sidebar_accent))
                    .hover(|d| d.bg(theme.sidebar_accent))
                    .child(div().size(px(7.)).flex_none().rounded_full().bg(color))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().overflow_hidden().whitespace_nowrap().text_ellipsis().text_sm().child(row.name.clone()))
                            .child(
                                div()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(detail),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        for fx in this.model.open_row(&action) {
                            this.effect(fx);
                        }
                        cx.notify();
                    })),
            );
        }
        if rows.is_empty() {
            section = section.child(div().px_2().pb_1().text_xs().text_color(theme.muted_foreground).child("No named sessions"));
        }
        section
    }

    fn render_rail(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let mut rail = v_flex()
            .w(px(250.))
            .h_full()
            .flex_none()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.sidebar_border)
            .child(
                h_flex()
                    .px_3()
                    .py_3()
                    .justify_between()
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("Synaps"))
                    .child(Button::new("new").ghost().compact().label("+ New").on_click(cx.listener(|this, _, _, _| this.create()))),
            );
        let mut list = v_flex()
            .id("sessions")
            .flex_1()
            .overflow_y_scroll()
            .px_2()
            .gap_0p5()
            .child(self.render_named(cx))
            .child(
                div()
                    .px_2()
                    .pt_2()
                    .pb_0p5()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.muted_foreground)
                    .child("SESSIONS"),
            );
        let mut unnamed = 0;
        for (i, s) in self.model.unnamed_sessions().enumerate() {
            unnamed += 1;
            let active = self.model.active.as_ref().is_some_and(|a| a.session_id == s.id);
            let id = s.id.clone();
            list = list.child(
                h_flex()
                    .id(("session", i))
                    .px_2()
                    .py_1p5()
                    .gap_2()
                    .rounded_md()
                    .cursor_pointer()
                    .when(active, |d| d.bg(theme.sidebar_accent))
                    .hover(|d| d.bg(theme.sidebar_accent))
                    .child(div().size(px(7.)).rounded_full().bg(if s.busy {
                        theme.warning
                    } else if s.lifecycle == "live" {
                        theme.success
                    } else {
                        theme.muted_foreground
                    }))
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_sm()
                            .child(s.label()),
                    )
                    .on_click(cx.listener(move |this, _, _, _| this.attach(id.clone()))),
            );
        }
        if unnamed == 0 {
            list = list.child(div().px_2().text_sm().text_color(theme.muted_foreground).child("No unnamed live sessions"));
        }
        rail = rail.child(list);
        rail
    }

    fn render_header(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (label, color): (SharedString, Hsla) = match &self.model.conn {
            _ if self.model.reattaching => ("● reattaching".into(), theme.warning),
            ConnState::Connected(v) => (format!("● connected · {v}").into(), theme.success),
            ConnState::Connecting => ("● connecting".into(), theme.warning),
            ConnState::Reconnecting => ("● reconnecting".into(), theme.warning),
            ConnState::Off(e) => (format!("● {}", one_line(e, 60)).into(), theme.danger),
            ConnState::Incompatible(e) => (format!("● {}", one_line(e, 60)).into(), theme.danger),
        };
        let title: SharedString = self.model.transcript.title().unwrap_or("No session").to_string().into();
        let attached = self.model.active.is_some() && self.client.is_some();
        h_flex()
            .px_4()
            .py_2()
            .gap_3()
            .border_b_1()
            .border_color(theme.border)
            .when_some(self.model.active_name(), |d, n| {
                d.child(
                    div()
                        .px_2()
                        .py_0p5()
                        .rounded_md()
                        .bg(theme.secondary)
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(n.to_string()),
                )
            })
            .child(div().flex_1().overflow_hidden().whitespace_nowrap().text_ellipsis().child(title))
            .when(attached, |d| {
                d.child(Button::new("name-session").ghost().compact().label("Name this session…").on_click(cx.listener(
                    |this, _, window, cx| {
                        this.model.toggle_naming();
                        if this.model.named.naming {
                            let current = this.model.active_name().unwrap_or_default().to_string();
                            this.name_input.update(cx, |s, cx| {
                                s.set_value(current, window, cx);
                                s.focus(window, cx);
                            });
                        }
                        cx.notify();
                    },
                )))
            })
            .when(self.model.transcript.cost_usd() > 0.0, |d| {
                d.child(div().text_sm().text_color(theme.muted_foreground).child(format!("${:.4}", self.model.transcript.cost_usd())))
            })
            .child(
                div()
                    .px_2()
                    .py_0p5()
                    .rounded_full()
                    .border_1()
                    .border_color(theme.border)
                    .text_xs()
                    .text_color(color)
                    .child(label),
            )
    }
}

impl AppView {
    /// The "Name this session…" row under the header.
    fn render_naming(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let pending = self.model.named.rename.is_some();
        let mut row = v_flex().px_4().py_2().gap_1().border_b_1().border_color(theme.border).child(
            h_flex()
                .gap_2()
                .items_center()
                .child(div().text_sm().text_color(theme.muted_foreground).child("Name"))
                .child(div().flex_1().child(Input::new(&self.name_input)))
                .child(
                    Button::new("name-save")
                        .primary()
                        .compact()
                        .disabled(pending)
                        .label(if pending { "Saving…" } else { "Save" })
                        .on_click(cx.listener(|this, _, _, cx| {
                            let typed = this.name_input.read(cx).value().to_string();
                            let req = this.model.rename_request(&typed);
                            this.rename(req);
                            cx.notify();
                        })),
                )
                .child(Button::new("name-cancel").ghost().compact().label("Cancel").on_click(cx.listener(|this, _, _, cx| {
                    this.model.named.naming = false;
                    cx.notify();
                }))),
        );
        if let Some(e) = &self.model.named.rename_error {
            row = row.child(div().text_xs().text_color(theme.danger).child(e.clone()));
        }
        // Only for the session it was made for (a switch drops it).
        if let Some(r) = self.model.named.replace.as_ref().filter(|r| self.model.active.as_ref().is_some_and(|a| a.session_id == r.session_id)) {
            row = row.child(
                Button::new("name-replace")
                    .danger()
                    .compact()
                    .disabled(pending)
                    .label(format!("Replace '{}' with '{}'", r.current, r.asked))
                    .on_click(cx.listener(|this, _, _, cx| {
                        let req = this.model.replace_request();
                        this.rename(req);
                        cx.notify();
                    })),
            );
        }
        row
    }
}

fn one_line(s: &str, max: usize) -> String {
    let line = s.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    let more = s.lines().filter(|l| !l.trim().is_empty()).count() > 1;
    let mut out: String = line.chars().take(max).collect();
    if more || line.chars().count() > max {
        out.push_str(" …");
    }
    out
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
        let theme = cx.theme();
        let busy = self.model.transcript.busy();
        let can_send = self.model.can_send();
        let prompts = self.model.transcript.pending_prompts().len();
        let list = list(self.list.clone(), cx.processor(|this, ix, _window, cx| this.render_row(ix, cx)))
            .size_full();
        let main = v_flex()
            .flex_1()
            .h_full()
            .min_w_0()
            .child(self.render_header(cx))
            .when(self.model.named.naming && self.model.active.is_some(), |d| d.child(self.render_naming(cx)))
            .child(div().flex_1().min_h_0().child(list))
            .when(prompts > 0, |d| {
                let p = &self.model.transcript.pending_prompts()[0];
                d.child(
                    div()
                        .mx_6()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(theme.warning.opacity(0.15))
                        .text_sm()
                        .child(format!("{}: {}  (answer in the TUI; prompts are not wired here yet)", p.title, p.prompt)),
                )
            })
            .when_some(self.model.last_error.clone(), |d, e| {
                d.child(div().px_6().text_xs().text_color(theme.danger).child(e))
            })
            .child(
                h_flex()
                    .px_6()
                    .py_3()
                    .gap_2()
                    .items_end()
                    .child(div().flex_1().child(Textarea::new(&self.composer).disabled(!can_send)))
                    .child(if busy {
                        Button::new("abort").danger().label("Abort").on_click(cx.listener(|this, _, _, _| {
                            if let Some(c) = &this.client {
                                let _ = c.abort();
                            }
                        }))
                    } else {
                        Button::new("send").primary().label("Send").on_click(cx.listener(|this, _, window, cx| {
                            let text = this.composer.read(cx).value().trim().to_string();
                            if !text.is_empty() && this.submit(text) {
                                this.composer.update(cx, |s, cx| s.set_value("", window, cx));
                            }
                        }))
                    }),
            );
        h_flex()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(self.render_rail(cx))
            .child(main)
    }
}
