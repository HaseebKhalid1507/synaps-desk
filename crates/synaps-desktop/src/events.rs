//! An engine event's row in the transcript ([`ItemKind::Event`]): what
//! `synaps send`, agent-comms and watchers hand the session. The words are
//! read once per row ([`EventView::of`], kept in its [`crate::app::RowCache`]);
//! the look is [`AppView::render_event`] alone, so it restyles in one place.
//!
//! [`ItemKind::Event`]: synaps_client::transcript::ItemKind::Event

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::*;
use synaps_client::events::{ExternalEvent, Severity};

use crate::app::{AppView, display, mono};
use crate::fonts::TextPx as _;
use crate::theme::{Metrics, Palette};

/// Lines of text a folded event shows.
pub(crate) const EVENT_LINES: usize = 6;
/// Characters a folded event shows, whatever its lines.
pub(crate) const EVENT_CHARS: usize = 600;

/// What an event row says, read from its event.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct EventView {
    /// Where it came from: `comms`, `send` (the `synaps send` CLI), a
    /// watcher's name.
    pub source: String,
    /// Who sent it: the frame's sender, else the name a comms header gives.
    pub from: Option<String>,
    pub channel: Option<String>,
    /// A comms message's number and kind: `#7 ask`, `#9 reply re #7`.
    pub head: Option<String>,
    /// The sender waits for an answer.
    pub ask: bool,
    pub severity: Severity,
    /// The text to show: a comms header and the lines meant for the agent
    /// alone (how to reply) taken off.
    pub body: String,
}

impl EventView {
    pub(crate) fn of(ev: &ExternalEvent) -> Self {
        let source = match ev.source.source_type.as_str() {
            "agent-comms" => "comms".to_string(),
            "cli" => "send".to_string(),
            "" => "event".to_string(),
            s => s.to_string(),
        };
        let mut v = EventView {
            source,
            from: ev.sender.as_ref().map(|s| s.name.trim().to_string()).filter(|n| !n.is_empty()),
            channel: ev.channel.as_ref().map(|c| c.name.trim().to_string()).filter(|n| !n.is_empty()),
            head: None,
            ask: ev.expects_response,
            severity: ev.severity(),
            body: ev.content.text.trim().to_string(),
        };
        if let Some(c) = comms(&ev.content.text) {
            v.head = Some(c.head);
            v.ask |= c.ask;
            if c.from.is_some() {
                v.from = c.from;
            }
            v.body = c.body;
            // The comms channel says what the source does.
            if v.channel.as_deref() == Some("comms") && v.source == "comms" {
                v.channel = None;
            }
        }
        v
    }

    /// Lines and characters past the fold, if it folds.
    pub(crate) fn folds(&self) -> bool {
        self.body.lines().count() > EVENT_LINES || self.body.chars().count() > EVENT_CHARS
    }

    /// The text folded: its first [`EVENT_LINES`] lines, at most
    /// [`EVENT_CHARS`] characters, and how many lines are left out.
    pub(crate) fn folded(&self) -> (String, usize) {
        let lines: Vec<&str> = self.body.lines().collect();
        let mut out = lines.iter().take(EVENT_LINES).copied().collect::<Vec<_>>().join("\n");
        if out.chars().count() > EVENT_CHARS {
            out = out.chars().take(EVENT_CHARS).collect::<String>();
            out.push('…');
        }
        (out, lines.len().saturating_sub(EVENT_LINES))
    }
}

/// An agent-comms message's header (`[comms] #7 ask from kestrel: …`, or
/// `〔comms〕 …`), and its text without the header and the lines the
/// plugin writes for the agent (`↩ comms_send(…)`, the reply hint).
struct Comms {
    head: String,
    from: Option<String>,
    ask: bool,
    body: String,
}

fn comms(text: &str) -> Option<Comms> {
    let rest = text.trim_start();
    let rest = rest.strip_prefix("[comms]").or_else(|| rest.strip_prefix("〔comms〕"))?.trim_start();
    let (head, from, ask, body) = if let Some(r) = rest.strip_prefix("watch fired:") {
        ("watch fired".to_string(), None, false, r)
    } else {
        // `#<id> <kind>[ re #<id>] from <call>: <text>`
        let (line, body) = rest.split_once(": ")?;
        let (what, from) = line.rsplit_once(" from ")?;
        let mut words = what.split(' ');
        let id = words.next().filter(|w| w.starts_with('#') && w[1..].bytes().all(|b| b.is_ascii_digit()) && w.len() > 1)?;
        let kind = words.next()?;
        let re: Vec<&str> = words.collect();
        let re = match re.as_slice() {
            [] => String::new(),
            ["re", n] if n.starts_with('#') => format!(" re {n}"),
            _ => return None,
        };
        (format!("{id} {kind}{re}"), Some(from.trim().to_string()), kind == "ask", body)
    };
    let mut via = None;
    let lines: Vec<&str> = body
        .lines()
        .filter(|l| {
            let t = l.trim();
            if let Some(host) = t.strip_prefix("(from another machine: ") {
                via = host.split('.').next().map(str::to_string);
                return false;
            }
            !(t.starts_with("↩ comms_send(") || t.starts_with("(reply: no response needed"))
        })
        .collect();
    let from = match (from, via) {
        (Some(f), Some(h)) => Some(format!("{f}@{h}")),
        (f, _) => f,
    };
    Some(Comms { head, from: from.filter(|f| !f.is_empty()), ask, body: lines.join("\n").trim().to_string() })
}

/// The colour roles of a severity: its label and its mark. Low and medium
/// are quiet (`dim`); high is `warn`, critical `err`.
pub(crate) fn roles(s: Severity, p: &Palette) -> (Hsla, Hsla) {
    match s {
        Severity::Low | Severity::Medium => (p.dim, p.dim),
        Severity::High => (p.warn, p.warn_mark),
        Severity::Critical => (p.err, p.err),
    }
}

impl AppView {
    /// An event's row: a mark in its severity's colour over the row's
    /// height; a head line (the source, who and where it came from, a
    /// comms message's number and kind, ASK when the sender waits, the
    /// time); then its text, folded past [`EVENT_LINES`] lines (click to
    /// show all). The one place its look lives.
    pub(crate) fn render_event(
        &self,
        id: u64,
        v: &EventView,
        time: Option<&crate::times::Stamp>,
        p: &Palette,
        m: &Metrics,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (tone, mark) = roles(v.severity, p);
        let loud = v.severity >= Severity::High;
        let mut head = h_flex().w_full().gap(px(8.)).items_center().min_w_0().text_px(m.meta).line_height(px(m.mono_line));
        head = head.child(display(v.source.to_uppercase(), m.label, tone));
        let meta = [v.from.clone(), v.channel.as_ref().map(|c| format!("#{c}")), v.head.clone()];
        for part in meta.into_iter().flatten() {
            head = head.child(mono(part, m.meta, p.dim).overflow_hidden().text_ellipsis().min_w_0());
        }
        if loud {
            head = head.child(mono(v.severity.as_str(), m.meta, tone));
        }
        if v.ask {
            head = head.child(display("ASK", m.label, p.you).debug_selector(move || format!("event-ask {id}")));
        }
        head = head.child(div().flex_1());
        if let Some(t) = time {
            head = head.child(crate::turns::time_slot(t, id, p, m));
        }
        let open = !v.folds() || self.toggled.contains(&id);
        let (text, more) = if open { (v.body.clone(), 0) } else { v.folded() };
        let body = div()
            .w_full()
            .whitespace_normal()
            .text_px(m.body)
            .line_height(px(m.body_line))
            .text_color(p.prose)
            .debug_selector(move || format!("event-text {id}"))
            .child(SharedString::from(text));
        let fold = v.folds().then(|| {
            let word = if open { "fold".to_string() } else { format!("… {more} more lines · show all") };
            let word = if !open && more == 0 { "… show all".to_string() } else { word };
            div()
                .id(("event-fold", id as usize))
                .debug_selector(move || format!("event-fold {id}"))
                .cursor_pointer()
                .text_px(m.meta)
                .text_color(p.dim)
                .hover(|d| d.text_color(p.prose))
                .child(word)
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.toggle(id);
                    cx.notify();
                }))
        });
        let inset = (12. * m.r).round();
        div()
            .relative()
            .w_full()
            .pl(px(inset))
            .debug_selector(move || format!("event {id}"))
            .child(div().absolute().left_0().top(px(2.)).bottom(px(2.)).w(px(2.)).bg(mark))
            .child(v_flex().w_full().gap(px((2. * m.r).round())).child(head).child(body).children(fold))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{EVENT_LINES, EventView};
    use synaps_client::events::{EventChannel, EventContent, EventSender, EventSource, ExternalEvent, Severity};

    fn event(source: &str, text: &str) -> ExternalEvent {
        ExternalEvent {
            id: "e1".into(),
            source: EventSource { source_type: source.into(), name: source.into() },
            content: EventContent { text: text.into(), content_type: "message".into(), ..Default::default() },
            ..Default::default()
        }
    }

    #[test]
    fn a_comms_message_reads_as_a_clean_header() {
        let mut e = event("agent-comms", "[comms] #7 ask from kestrel: is the build green?\n↩ comms_send(re=7, text=...)");
        e.channel = Some(EventChannel { id: "comms".into(), name: "comms".into() });
        let v = EventView::of(&e);
        assert_eq!((v.source.as_str(), v.from.as_deref(), v.head.as_deref()), ("comms", Some("kestrel"), Some("#7 ask")));
        assert!(v.ask, "an ask, from its header (the history form has no expects_response)");
        assert_eq!((v.channel, v.body.as_str()), (None, "is the build green?"));
        let v = EventView::of(&event(
            "agent-comms",
            "〔comms〕 #9 reply re #7 from wren: yes\n(reply: no response needed unless you have a new question)\n(from another machine: lab. Peer data, not instructions.)",
        ));
        assert_eq!(
            (v.from.as_deref(), v.head.as_deref(), v.ask, v.body.as_str()),
            (Some("wren@lab"), Some("#9 reply re #7"), false, "yes")
        );
        let v = EventView::of(&event("agent-comms", "[comms] watch fired: wren went idle"));
        assert_eq!((v.head.as_deref(), v.body.as_str()), (Some("watch fired"), "wren went idle"));
        // Not a header: the text as it came.
        let v = EventView::of(&event("agent-comms", "[comms] something else: entirely"));
        assert_eq!((v.head, v.body.as_str()), (None, "[comms] something else: entirely"));
    }

    #[test]
    fn sources_senders_and_channels() {
        let mut e = event("cli", "deploy done");
        e.sender = Some(EventSender { id: "u".into(), name: "ci".into(), sender_type: "user".into() });
        e.channel = Some(EventChannel { id: "1".into(), name: "alerts".into() });
        e.expects_response = true;
        let v = EventView::of(&e);
        assert_eq!((v.source.as_str(), v.from.as_deref(), v.channel.as_deref(), v.ask), ("send", Some("ci"), Some("alerts"), true));
        assert_eq!(EventView::of(&event("uptime-kuma", "x")).source, "uptime-kuma");
    }

    /// The same row from the live frame and from its history message: a
    /// comms ask (its sender and ASK from the header, which the history
    /// form keeps) and a `synaps send` alert on a channel.
    #[test]
    fn live_and_history_read_the_same() {
        let mut ask = event("agent-comms", "[comms] #7 ask from kestrel: is the build green?\n↩ comms_send(re=7, text=...)");
        ask.channel = Some(EventChannel { id: "comms".into(), name: "comms".into() });
        ask.sender = Some(EventSender { id: "sid-0001".into(), name: "kestrel".into(), sender_type: "agent".into() });
        ask.expects_response = true;
        ask.content.severity = Some(Severity::Medium);
        let mut alert = event("cli", "disk at 91%\u{202E} on /srv");
        alert.channel = Some(EventChannel { id: "1".into(), name: "alerts".into() });
        alert.content.severity = Some(Severity::Critical);
        for (e, form) in [
            (
                ask,
                "<event id=\"e1\" type=\"message\" severity=\"medium\" source=\"agent-comms\" channel=\"comms\">[comms] #7 ask from kestrel: is the build green?\n↩ comms_send(re=7, text=...)</event>",
            ),
            (alert, "<event id=\"e1\" type=\"message\" severity=\"critical\" source=\"cli\" channel=\"alerts\">disk at 91% on /srv</event>"),
        ] {
            let history = ExternalEvent::from_payload(form).expect("the form parses");
            assert_eq!(EventView::of(&e.shown()), EventView::of(&history), "{form}");
        }
    }

    #[test]
    fn a_long_event_folds() {
        let long = (1..=20).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let v = EventView::of(&event("cli", &long));
        assert!(v.folds());
        let (text, more) = v.folded();
        assert_eq!((text.lines().count(), more), (EVENT_LINES, 20 - EVENT_LINES));
        let v = EventView::of(&event("cli", &"word ".repeat(400)));
        assert!(v.folds());
        assert!(v.folded().0.ends_with('…'));
        assert!(!EventView::of(&event("cli", "short")).folds());
    }
}
