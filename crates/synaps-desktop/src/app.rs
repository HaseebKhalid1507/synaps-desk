//! The main window: session rail, virtualized transcript, composer.

use std::collections::HashSet;
use std::time::Instant;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::text::TextView;
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::*;
use gpui_kit::prelude::FluentBuilder as _;
use synaps_client::protocol::SessionMeta;
use synaps_client::transcript::{ItemKind, ToolStatus, Transcript};
use synaps_client::{Client, Event, Status};

use crate::bench::BenchState;

/// Per-row strings derived from a transcript item, rebuilt only when the
/// item's `rev` changes (so a frame never re-allocates unchanged rows).
struct RowCache {
    id: u64,
    rev: u64,
    text: SharedString,
    detail: SharedString,
}

#[derive(Clone)]
enum Conn {
    Off(SharedString),
    Connecting,
    Connected(SharedString),
    Reconnecting,
}

pub struct AppView {
    pub client: Option<Client>,
    conn: Conn,
    sessions: Vec<SessionMeta>,
    pub active: Option<String>,
    pub transcript: Transcript,
    rows: Vec<RowCache>,
    pub list: ListState,
    expanded: HashSet<u64>,
    composer: Entity<TextareaState>,
    /// Events received since the last frame: applied once per frame in
    /// `render`, so a token stream costs one layout per frame, not per token.
    pending: Vec<Event>,
    last_error: Option<SharedString>,
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
        let subs = vec![cx.subscribe_in(&composer, window, |this: &mut Self, input, ev: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { shift: false, .. } = ev {
                let text = input.read(cx).value().trim().to_string();
                if !text.is_empty() && this.submit(text) {
                    input.update(cx, |s, cx| s.set_value("", window, cx));
                }
            }
        })];
        let (client, conn) = match client {
            Some((c, rx)) => {
                let mut v = Self::blank(composer, Some(c), Conn::Connecting, subs);
                v.pump(rx, cx);
                return v;
            }
            None => (None, Conn::Off("offline".into())),
        };
        Self::blank(composer, client, conn, subs)
    }

    fn blank(composer: Entity<TextareaState>, client: Option<Client>, conn: Conn, subs: Vec<Subscription>) -> Self {
        let list = ListState::new(0, ListAlignment::Top, px(600.));
        list.set_follow_mode(FollowMode::Tail);
        Self {
            client,
            conn,
            sessions: Vec::new(),
            active: None,
            transcript: Transcript::new(),
            rows: Vec::new(),
            list,
            expanded: HashSet::new(),
            composer,
            pending: Vec::new(),
            last_error: None,
            bench: None,
            plain_rows: false,
            _tasks: Vec::new(),
            _subs: subs,
        }
    }

    /// Drain an event channel into `pending`; notify once per frame.
    pub fn pump(&mut self, rx: async_channel::Receiver<Event>, cx: &mut Context<Self>) {
        let task = cx.spawn(async move |this, cx| {
            while let Ok(first) = rx.recv().await {
                let mut batch = vec![first];
                while let Ok(e) = rx.try_recv() {
                    batch.push(e);
                }
                let now = Instant::now();
                let alive = this.update(cx, |this, cx| {
                    if let Some(b) = this.bench.as_mut() {
                        b.on_received(&batch, now);
                    }
                    let was_empty = this.pending.is_empty();
                    this.pending.extend(batch);
                    if was_empty {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        });
        self._tasks.push(task);
    }

    fn submit(&mut self, text: String) -> bool {
        match (&self.client, &self.active) {
            (Some(c), Some(_)) => {
                if self.transcript.busy {
                    c.send(synaps_client::Command::Steer { text });
                } else {
                    c.submit(text);
                }
                true
            }
            _ => {
                self.last_error = Some("not attached to a session".into());
                false
            }
        }
    }

    fn apply_pending(&mut self) {
        for ev in std::mem::take(&mut self.pending) {
            match ev {
                Event::Status(s) => {
                    self.conn = match s {
                        Status::Connecting => Conn::Connecting,
                        Status::Connected { daemon_version, profile, .. } => Conn::Connected(
                            format!("{}{}", daemon_version, profile.map(|p| format!(" · {p}")).unwrap_or_default()).into(),
                        ),
                        Status::Reconnecting { .. } => Conn::Reconnecting,
                        Status::Offline(e) => Conn::Off(e.into()),
                    }
                }
                Event::Sessions(s) => self.sessions = s,
                Event::Attached { attached, .. } => {
                    self.active = Some(attached.meta.id.clone());
                    self.transcript.load_attached(&attached);
                    if let Some(c) = &self.client {
                        c.list_sessions();
                    }
                    self.list.set_follow_mode(FollowMode::Tail);
                }
                Event::Session { session_id, event, .. } => {
                    if self.active.as_deref() == Some(session_id.as_str()) {
                        self.transcript.apply(&event);
                    }
                }
                Event::Detached { reason, .. } => {
                    self.last_error = Some(format!("detached: {reason}").into());
                    self.transcript.busy = false;
                }
                Event::Error(e) => self.last_error = Some(e.into()),
            }
        }
        self.sync_list();
    }

    /// Push transcript changes into the virtual list: only the changed tail
    /// is spliced (and so re-measured).
    pub fn sync_list(&mut self) {
        let Some(sp) = self.transcript.take_splice() else { return };
        self.rows.truncate(sp.start);
        for item in &self.transcript.items[sp.start..] {
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
        debug_assert!(self.rows.iter().zip(&self.transcript.items).all(|(r, i)| r.id == i.id && r.rev == i.rev));
        self.list.splice(sp.start..sp.old_len, sp.new_len - sp.start);
    }

    fn render_row(&self, ix: usize, cx: &Context<Self>) -> AnyElement {
        let (Some(item), Some(row)) = (self.transcript.items.get(ix), self.rows.get(ix)) else {
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
                                if let Some(ix) = this.transcript.items.iter().position(|i| i.id == id) {
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
                    .child(Button::new("new").ghost().compact().label("+ New").on_click(cx.listener(|this, _, _, _| {
                        if let Some(c) = &this.client {
                            c.create(None);
                        }
                    }))),
            );
        let mut list = v_flex().id("sessions").flex_1().overflow_y_scroll().px_2().gap_0p5();
        for (i, s) in self.sessions.iter().enumerate() {
            let active = self.active.as_deref() == Some(s.id.as_str());
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
                    .on_click(cx.listener(move |this, _, _, _| {
                        if let Some(c) = &this.client {
                            c.attach(id.clone());
                        }
                    })),
            );
        }
        if self.sessions.is_empty() {
            list = list.child(div().px_2().text_sm().text_color(theme.muted_foreground).child("No live sessions"));
        }
        rail = rail.child(list);
        rail
    }

    fn render_header(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (label, color): (SharedString, Hsla) = match &self.conn {
            Conn::Connected(v) => (format!("● connected · {v}").into(), theme.success),
            Conn::Connecting => ("● connecting".into(), theme.warning),
            Conn::Reconnecting => ("● reconnecting".into(), theme.warning),
            Conn::Off(e) => (format!("● {}", one_line(e, 60)).into(), theme.danger),
        };
        let title: SharedString = self.transcript.title.clone().unwrap_or_else(|| "No session".into()).into();
        h_flex()
            .px_4()
            .py_2()
            .gap_3()
            .border_b_1()
            .border_color(theme.border)
            .child(div().flex_1().overflow_hidden().whitespace_nowrap().text_ellipsis().child(title))
            .when(self.transcript.cost_usd > 0.0, |d| {
                d.child(div().text_sm().text_color(theme.muted_foreground).child(format!("${:.4}", self.transcript.cost_usd)))
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
        if !self.pending.is_empty() {
            self.apply_pending();
        }
        if let Some(b) = self.bench.as_mut() {
            b.on_render(&self.list, window);
        }
        let theme = cx.theme();
        let busy = self.transcript.busy;
        let prompts = self.transcript.pending_prompts.len();
        let list = list(self.list.clone(), cx.processor(|this, ix, _window, cx| this.render_row(ix, cx)))
            .size_full();
        let main = v_flex()
            .flex_1()
            .h_full()
            .min_w_0()
            .child(self.render_header(cx))
            .child(div().flex_1().min_h_0().child(list))
            .when(prompts > 0, |d| {
                let p = &self.transcript.pending_prompts[0];
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
            .when_some(self.last_error.clone(), |d, e| {
                d.child(div().px_6().text_xs().text_color(theme.danger).child(e))
            })
            .child(
                h_flex()
                    .px_6()
                    .py_3()
                    .gap_2()
                    .items_end()
                    .child(div().flex_1().child(Textarea::new(&self.composer)))
                    .child(if busy {
                        Button::new("abort").danger().label("Abort").on_click(cx.listener(|this, _, _, _| {
                            if let Some(c) = &this.client {
                                c.abort();
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
