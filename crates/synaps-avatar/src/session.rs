//! The session link: one Synaps session drives the body. Pure state, no GPUI: daemon
//! events go in, client actions and a body snapshot come out. The view applies the
//! snapshot (a mood only when it changes, the reply into the bubble, the pending prompt)
//! and runs the actions on the `Client`.
//!
//! Rules from lynx's client notes: attach with `Mirror` (Observe refuses input), drop
//! every event whose `attach_seq` is not the current one, treat `Attached` as a full
//! replace (also after a reload), rebuild on a history `Replace` and `resync` when the
//! transcript refuses an update, and hand refused input back to the user.

use crate::anim::Mood;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use synaps_client::protocol::{LlmEvent, PromptRequest, SessionStreamEvent, StreamEvent};
use synaps_client::transcript::ItemKind;
use synaps_client::{AttachMode, CreateOptions, Cwd, Error, Event, SessionEvent, SessionMeta, Status, Transcript};

/// The session the default target attaches to, or creates.
pub const DEFAULT_NAME: &str = "avatar";
/// Done shows this long after a turn, then Idle.
const DONE_FOR: Duration = Duration::from_secs(3);
/// Annoyed (abort, error) shows this long.
const ANNOYED_FOR: Duration = Duration::from_secs(5);
/// Before the first attach, stay as we are this long instead of flashing Away.
const BOOT_GRACE: Duration = Duration::from_millis(2_500);
/// After a detach, look for the session again this soon; while it is missing, this often.
const RETRY_SOON: Duration = Duration::from_secs(2);
const RETRY_SLOW: Duration = Duration::from_secs(10);
/// Longest tool name shown in the tag.
const TOOL_MAX: usize = 18;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// Attach to the live session named [`DEFAULT_NAME`], else create it.
    Default,
    /// Attach to this session (id, journal id or name). Never creates one.
    Session(String),
    /// Create a new session; afterwards follow it (and create again if it is gone).
    New,
}

#[derive(Clone, Debug)]
pub struct Options {
    pub target: Target,
    pub cwd: Option<PathBuf>,
    pub model: Option<String>,
    pub sleepy_after: Duration,
}

/// What the view should do on the client.
#[derive(Debug, Clone)]
pub enum Act {
    Attach(String),
    Create(CreateOptions),
    Resume(String, CreateOptions),
    List,
    Resync,
}

/// One-off effects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Once {
    /// Re-attached after a loss: play the appear glitch.
    Appear,
    /// Input the daemon refused: give it back to the user.
    Restore(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptView {
    pub id: u64,
    pub title: String,
    pub text: String,
    /// A secret (password, API key): masked input. Otherwise a yes/no confirm.
    pub secret: bool,
}

/// What the body shows, derived from the session state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Body {
    pub mood: Mood,
    /// Status label override (`WORKING · bash`, `COMPACTING`).
    pub label: Option<String>,
    /// The reply of the current (or last) turn, cleaned for the bubble, plus a notice.
    pub reply: String,
    /// Bumps for every new reply: a new bubble.
    pub turn: u64,
    /// The turn still runs (more text may come).
    pub busy: bool,
    /// The user's line of the running turn.
    pub echo: Option<String>,
    pub prompt: Option<PromptView>,
    /// Attached and connected: input can go out.
    pub live: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Attach {
    None,
    Pending(u64),
    Live(u64),
    Reattaching(u64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Phase {
    Thinking,
    Talking,
    Working(String),
}

pub struct Link {
    opts: Options,
    started: Instant,
    connected: bool,
    attach: Attach,
    ever_live: bool,
    /// Lost a live attachment since: the next attach plays the appear glitch.
    lost: bool,
    /// The session we follow once attached (or the one `--session` named).
    follow: Option<String>,
    /// The pending attach is a create (named or not) or a resume of the default name.
    creating: Option<Creating>,
    busy: bool,
    phase: Phase,
    compacting: bool,
    prompts: Vec<PromptRequest>,
    /// This turn's assistant text, raw.
    raw: String,
    /// Where the current response's text starts in `raw` (a reset drops it).
    response_at: usize,
    notice: Option<String>,
    turn: u64,
    echo: Option<String>,
    flash: Option<(Mood, Instant)>,
    last_active: Instant,
    retry_at: Option<Instant>,
    /// Sent but not yet seen as a turn or steer: comes back if refused.
    pending_input: Option<String>,
    once: Vec<Once>,
    transcript: Transcript,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Creating {
    Named,
    Resume,
    Unnamed,
}

impl Link {
    pub fn new(opts: Options, now: Instant) -> Link {
        let follow = match &opts.target {
            Target::Session(s) => Some(s.clone()),
            _ => None,
        };
        Link {
            opts,
            started: now,
            connected: false,
            attach: Attach::None,
            ever_live: false,
            lost: false,
            follow,
            creating: None,
            busy: false,
            phase: Phase::Thinking,
            compacting: false,
            prompts: Vec::new(),
            raw: String::new(),
            response_at: 0,
            notice: None,
            turn: 0,
            echo: None,
            flash: None,
            last_active: now,
            retry_at: None,
            pending_input: None,
            once: Vec::new(),
            transcript: Transcript::new(),
        }
    }

    fn current(&self) -> Option<u64> {
        match self.attach {
            Attach::None => None,
            Attach::Pending(s) | Attach::Live(s) | Attach::Reattaching(s) => Some(s),
        }
    }

    pub fn live(&self) -> bool {
        self.connected && matches!(self.attach, Attach::Live(_))
    }

    pub fn busy(&self) -> bool {
        self.busy
    }

    /// The view sent an attach/create/resume that returned `seq`.
    pub fn attaching(&mut self, seq: u64) {
        self.attach = Attach::Pending(seq);
    }

    /// The user's text went out (submit or steer).
    pub fn submitted(&mut self, text: String) {
        self.pending_input = Some(text);
    }

    pub fn take_once(&mut self) -> Vec<Once> {
        std::mem::take(&mut self.once)
    }

    pub fn on_event(&mut self, ev: Event, now: Instant) -> Vec<Act> {
        if let Some(s) = ev.attach_seq()
            && Some(s) != self.current()
        {
            return Vec::new(); // stale: an attachment we have left
        }
        let mut acts = Vec::new();
        match ev {
            Event::Status(s) => {
                let up = matches!(s, Status::Connected { .. });
                if self.connected && !up && matches!(self.attach, Attach::Live(_)) {
                    self.lost = true;
                }
                self.connected = up;
            }
            Event::Sessions(list) => {
                if self.connected && self.attach == Attach::None && self.retry_at.is_none_or(|t| t <= now) {
                    self.retry_at = None;
                    acts.extend(self.resolve(&list, now));
                }
            }
            Event::Attached { attach_seq, attached, reconnect, .. } => {
                if let Err(e) = self.transcript.load_attached(&attached) {
                    eprintln!("synaps-avatar: attach: {e}");
                }
                let again = reconnect || self.lost;
                self.attach = Attach::Live(attach_seq);
                self.follow = Some(attached.meta.id.clone());
                self.creating = None;
                self.ever_live = true;
                self.lost = false;
                self.busy = attached.streaming;
                self.phase = Phase::Thinking;
                self.compacting = false;
                self.prompts = attached.pending_prompts.clone();
                self.pending_input = None;
                self.flash = None;
                self.echo = None;
                self.notice = None;
                self.last_active = now;
                // The last reply stays reachable from the chip.
                let items = self.transcript.items();
                let mut parts: Vec<&str> = items
                    .iter()
                    .rev()
                    .take_while(|i| i.kind != ItemKind::User)
                    .filter(|i| i.kind == ItemKind::Assistant)
                    .map(|i| i.text.as_str())
                    .collect();
                parts.reverse();
                self.raw = parts.join("\n\n");
                self.response_at = self.raw.len();
                self.turn += 1;
                if attached.rejected.total() > 0 {
                    eprintln!("synaps-avatar: attach snapshot: {} entries did not decode", attached.rejected);
                }
                if again {
                    self.once.push(Once::Appear);
                }
            }
            Event::Reattaching { attach_seq, reason, .. } => {
                eprintln!("synaps-avatar: session connection lost ({reason}); re-attaching");
                self.attach = Attach::Reattaching(attach_seq);
                self.lost = true;
                if let Some(t) = self.pending_input.take() {
                    self.once.push(Once::Restore(t));
                }
            }
            Event::Detached { reason, .. } => {
                eprintln!("synaps-avatar: detached: {reason}");
                let was = self.attach;
                self.attach = Attach::None;
                if matches!(was, Attach::Live(_) | Attach::Reattaching(_)) {
                    self.lost = true;
                }
                // A failed create of the default name: resume it, then create one unnamed.
                if matches!(was, Attach::Pending(_)) {
                    self.creating = match self.creating {
                        Some(Creating::Named) => Some(Creating::Resume),
                        Some(Creating::Resume) => Some(Creating::Unnamed),
                        c => c,
                    };
                }
                self.busy = false;
                self.compacting = false;
                self.prompts.clear();
                self.echo = None;
                if let Some(t) = self.pending_input.take() {
                    self.once.push(Once::Restore(t));
                }
                self.retry_at = Some(now + RETRY_SOON);
            }
            Event::History { update, .. } => {
                if self.transcript.apply_update(&update).is_err() {
                    acts.push(Act::Resync);
                }
            }
            Event::Session { event, .. } => {
                self.transcript.apply(&event);
                self.session_event(event, now);
            }
            Event::Error { error, .. } => match error {
                Error::Refused { command, reason } => {
                    eprintln!("synaps-avatar: {} refused: {reason}", command.as_deref().unwrap_or("command"));
                    if let Some(t) = self.pending_input.take() {
                        self.once.push(Once::Restore(t));
                    }
                }
                Error::Decode(e) => eprintln!("synaps-avatar: session event did not decode: {e}"),
                other => eprintln!("synaps-avatar: {other}"),
            },
            Event::CwdFallback { missing, .. } => eprintln!("synaps-avatar: cwd {missing} is gone on the daemon's machine; running in ~"),
            _ => {}
        }
        acts
    }

    fn session_event(&mut self, ev: SessionEvent, now: Instant) {
        match ev {
            SessionEvent::TurnStarted { user_text, .. } => {
                self.busy = true;
                self.phase = Phase::Thinking;
                self.raw.clear();
                self.response_at = 0;
                self.notice = None;
                self.flash = None;
                self.turn += 1;
                self.echo = user_text.map(|t| one_line(&t, 200)).filter(|t| !t.is_empty());
                self.pending_input = None;
                self.last_active = now;
            }
            SessionEvent::Stream { event } => match event {
                StreamEvent::Llm(l) => match l {
                    LlmEvent::Thinking { .. } => {
                        if !matches!(self.phase, Phase::Working(_)) {
                            self.phase = Phase::Thinking;
                        }
                    }
                    LlmEvent::ResponseStart => {
                        self.phase = Phase::Thinking;
                        self.response_at = self.raw.len();
                    }
                    LlmEvent::ResponseReset => {
                        self.raw.truncate(self.response_at);
                        self.phase = Phase::Thinking;
                    }
                    LlmEvent::Text { text } => {
                        if self.raw.len() == self.response_at && self.response_at > 0 && !text.trim().is_empty() {
                            self.raw.push_str("\n\n");
                            self.response_at = self.raw.len();
                        }
                        self.raw.push_str(&text);
                        self.phase = Phase::Talking;
                    }
                    LlmEvent::ToolUseStart { tool_name, .. } | LlmEvent::ToolUse { tool_name, .. } => {
                        self.phase = Phase::Working(tool_name);
                    }
                    LlmEvent::ToolResult { .. } => self.phase = Phase::Thinking,
                    _ => {}
                },
                StreamEvent::Session(SessionStreamEvent::Error { message }) => {
                    self.notice = Some(format!("‹error: {}›", one_line(&message, 90)));
                    self.flash = Some((Mood::Annoyed, now + ANNOYED_FOR));
                }
                _ => {}
            },
            SessionEvent::Idle => {
                self.busy = false;
                self.echo = None;
                self.last_active = now;
                if !self.flash.is_some_and(|f| f.0 == Mood::Annoyed) {
                    self.flash = Some((Mood::Done, now + DONE_FOR));
                }
            }
            SessionEvent::Aborted { .. } => {
                self.busy = false;
                self.echo = None;
                self.notice = Some("‹aborted›".into());
                self.flash = Some((Mood::Annoyed, now + ANNOYED_FOR));
                self.last_active = now;
            }
            SessionEvent::Prompt { request } => {
                self.prompts.retain(|p| p.id != request.id);
                self.prompts.push(request);
                self.last_active = now;
            }
            SessionEvent::PromptResolved { prompt_id } => self.prompts.retain(|p| p.id != prompt_id),
            SessionEvent::Steered { .. } | SessionEvent::Dequeued { .. } => self.pending_input = None,
            SessionEvent::CompactionStarted => self.compacting = true,
            SessionEvent::CompactionApplied { .. } | SessionEvent::CompactionCancelled => self.compacting = false,
            SessionEvent::CompactionFailed { message } => {
                self.compacting = false;
                eprintln!("synaps-avatar: compaction failed: {message}");
            }
            SessionEvent::Cleared { .. } => {
                self.raw.clear();
                self.response_at = 0;
                self.notice = None;
                self.turn += 1;
            }
            _ => {}
        }
    }

    /// Pick (or make) the session to attach to, from a fresh session list.
    fn resolve(&mut self, list: &[SessionMeta], now: Instant) -> Vec<Act> {
        let find = |key: &str| list.iter().find(|m| m.is(key) || m.name.as_deref() == Some(key)).map(|m| m.id.clone());
        if let Some(id) = self.follow.as_deref().and_then(find) {
            return vec![Act::Attach(id)];
        }
        match &self.opts.target {
            Target::Session(s) => {
                eprintln!("synaps-avatar: no live session `{s}`; looking again in {} s", RETRY_SLOW.as_secs());
                self.retry_at = Some(now + RETRY_SLOW);
                Vec::new()
            }
            Target::Default => {
                if let Some(id) = find(DEFAULT_NAME) {
                    return vec![Act::Attach(id)];
                }
                let c = self.creating.unwrap_or(Creating::Named);
                self.creating = Some(c);
                match c {
                    Creating::Named => vec![Act::Create(self.create_options().name(DEFAULT_NAME))],
                    Creating::Resume => vec![Act::Resume(DEFAULT_NAME.into(), self.create_options())],
                    Creating::Unnamed => vec![Act::Create(self.create_options())],
                }
            }
            Target::New => {
                self.creating = Some(Creating::Unnamed);
                vec![Act::Create(self.create_options())]
            }
        }
    }

    fn create_options(&self) -> CreateOptions {
        let mut o = CreateOptions::new().mode(AttachMode::Mirror);
        if let Some(c) = &self.opts.cwd {
            o = o.cwd(Cwd::Path(c.clone())).fallback_cwd(Cwd::Home);
        }
        if let Some(m) = &self.opts.model {
            o = o.model(m.clone());
        }
        o
    }

    /// Timers: retry a lost session.
    pub fn tick(&mut self, now: Instant) -> Vec<Act> {
        if self.retry_at.is_some_and(|t| t <= now) && self.attach == Attach::None && self.connected {
            // The answer (`Sessions`) resolves.
            self.retry_at = None;
            return vec![Act::List];
        }
        Vec::new()
    }

    /// The next instant the body or the link changes by itself.
    pub fn deadline(&self, now: Instant) -> Option<Instant> {
        let mut t: Vec<Instant> = Vec::new();
        if self.attach == Attach::None
            && self.connected
            && let Some(r) = self.retry_at
        {
            t.push(r.max(now));
        }
        if !self.ever_live {
            t.push(self.started + BOOT_GRACE);
        }
        if let Some((_, until)) = self.flash {
            t.push(until);
        }
        if self.live() && !self.busy {
            t.push(self.last_active + self.opts.sleepy_after);
        }
        t.into_iter().filter(|&d| d > now).min()
    }

    pub fn body(&self, now: Instant) -> Body {
        let mut label = None;
        let mood = if !self.live() {
            if !self.ever_live && now < self.started + BOOT_GRACE { Mood::Idle } else { Mood::Away }
        } else if !self.prompts.is_empty() {
            Mood::NeedsYou
        } else if self.compacting {
            label = Some("COMPACTING".into());
            Mood::Sleepy
        } else if self.busy {
            match &self.phase {
                Phase::Thinking => Mood::Thinking,
                Phase::Talking => Mood::Talking,
                Phase::Working(tool) => {
                    label = Some(format!("WORKING · {}", tool_label(tool)));
                    Mood::Working
                }
            }
        } else if let Some((m, until)) = self.flash
            && now < until
        {
            m
        } else if now >= self.last_active + self.opts.sleepy_after {
            Mood::Sleepy
        } else {
            Mood::Idle
        };
        let mut reply = clean(&self.raw);
        if let Some(n) = &self.notice {
            if !reply.is_empty() {
                reply.push('\n');
            }
            reply.push_str(n);
        }
        let prompt = self.prompts.first().map(|p| PromptView {
            id: p.id,
            title: one_line(&p.title, 60),
            text: one_line(&p.prompt, 400),
            secret: p.kind.as_str() != Some("confirm"),
        });
        Body {
            mood,
            label,
            reply,
            turn: self.turn,
            busy: self.busy,
            echo: self.echo.clone().filter(|_| self.busy),
            prompt,
            live: self.live(),
        }
    }
}

/// A tool name for the tag: short and safe (never arguments).
fn tool_label(name: &str) -> String {
    let n = name.rsplit("__").next().unwrap_or(name);
    let n: String = n.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':')).collect();
    if n.chars().count() > TOOL_MAX { format!("{}…", n.chars().take(TOOL_MAX - 1).collect::<String>()) } else { n }
}

/// Whitespace collapsed to single spaces, at most `max` chars (with an ellipsis).
pub fn one_line(s: &str, max: usize) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() > max { format!("{}…", s.chars().take(max - 1).collect::<String>().trim_end()) } else { s }
}

/// Markdown-lite for the bubble: fenced code becomes `‹code›`, `**`/`__` go, headings
/// lose their `#`s, bullets become `•`, links keep their text, inline code stays as is,
/// blank lines collapse (the bubble is small). Stable under streaming: cleaning a prefix
/// of the text gives (nearly always) a prefix of the clean text.
pub fn clean(raw: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut in_code = false;
    for line in raw.split('\n') {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            if !in_code {
                out.push("‹code›".into());
            }
            in_code = !in_code;
            continue;
        }
        if in_code {
            continue;
        }
        let l = match t.trim_start_matches('#') {
            rest if rest.len() < t.len() && (rest.is_empty() || rest.starts_with(' ')) => rest.trim_start(),
            _ => t,
        };
        let bullet = ["- ", "* ", "+ "].iter().find_map(|b| l.strip_prefix(b));
        let mut s = match bullet {
            Some(rest) => format!("• {rest}"),
            None => l.to_string(),
        };
        s = s.replace("**", "").replace("__", "");
        s = links(&s);
        let s = s.trim_end().to_string();
        if s.is_empty() {
            continue;
        }
        out.push(s);
    }
    out.join("\n")
}

/// `[text](url)` → `text`.
fn links(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('[') {
        let after = &rest[i + 1..];
        if let Some(j) = after.find("](")
            && let Some(k) = after[j + 2..].find(')')
            && !after[..j].contains('[')
        {
            out.push_str(&rest[..i]);
            out.push_str(&after[..j]);
            rest = &after[j + 2 + k + 1..];
        } else {
            out.push_str(&rest[..=i]);
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use synaps_client::protocol::{DaemonFrame, decode_frame};

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!("{}/../synaps-client/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
    }

    fn opts() -> Options {
        Options { target: Target::Default, cwd: None, model: None, sleepy_after: Duration::from_secs(600) }
    }

    fn attached(seq: u64, name: &str) -> Event {
        let DaemonFrame::Attached(a) = decode_frame(fixture(name).trim()).unwrap() else { panic!() };
        Event::Attached { attach_seq: seq, attached: a, mode: AttachMode::Mirror, reconnect: false }
    }

    fn turn(seq: u64) -> Vec<Event> {
        fixture("turn_events.ndjson")
            .lines()
            .map(|l| {
                let DaemonFrame::Event(env) = decode_frame(l).unwrap() else { panic!() };
                let event = env.decode().unwrap();
                Event::Session { attach_seq: seq, session_id: "s".into(), seq: env.seq, event, raw: Arc::new(env.event.clone()) }
            })
            .collect()
    }

    fn connected() -> Event {
        Event::Status(Status::Connected { daemon_version: "0.11.1".into(), profile: None, pid: 1, features: vec![] })
    }

    fn live_link(t0: Instant) -> Link {
        let mut l = Link::new(opts(), t0);
        l.on_event(connected(), t0);
        l.attaching(1);
        l.on_event(attached(1, "attached_create.json"), t0);
        assert!(l.live());
        l
    }

    /// The fixture's real turn (thinking, text, a bash tool, more text, idle) in moods.
    #[test]
    fn a_real_turn_drives_the_moods() {
        let t0 = Instant::now();
        let mut l = live_link(t0);
        assert_eq!(l.body(t0).mood, Mood::Idle);
        let mut seen: Vec<(Mood, Option<String>)> = Vec::new();
        for ev in turn(1) {
            l.on_event(ev, t0);
            let b = l.body(t0);
            if seen.last() != Some(&(b.mood, b.label.clone())) {
                seen.push((b.mood, b.label));
            }
        }
        let want = vec![
            (Mood::Idle, None),
            (Mood::Thinking, None),
            (Mood::Talking, None),
            (Mood::Working, Some("WORKING · bash".to_string())),
            (Mood::Thinking, None),
            (Mood::Talking, None),
            (Mood::Done, None),
        ];
        assert_eq!(seen, want);
        let b = l.body(t0);
        assert_eq!(b.reply, "Here's a simple Python hello world:\n‹code›\nNow running the echo command:\nDone! The echo command returned `hi` as expected.");
        assert!(!b.busy);
        assert_eq!(b.echo, None);
        // Done for ~3 s, then Idle; sleepy after the configured quiet time.
        assert_eq!(l.body(t0 + Duration::from_millis(2_900)).mood, Mood::Done);
        assert_eq!(l.body(t0 + Duration::from_millis(3_100)).mood, Mood::Idle);
        assert_eq!(l.body(t0 + Duration::from_secs(601)).mood, Mood::Sleepy);
        assert_eq!(l.deadline(t0), Some(t0 + DONE_FOR));
    }

    #[test]
    fn the_echo_shows_while_the_turn_runs() {
        let t0 = Instant::now();
        let mut l = live_link(t0);
        let evs = turn(1);
        l.on_event(evs[0].clone(), t0); // client_joined
        l.on_event(evs[1].clone(), t0); // turn_started
        let b = l.body(t0);
        assert!(b.busy);
        assert!(b.echo.as_deref().unwrap().starts_with("Think briefly, then write"));
        assert_eq!(b.reply, "");
    }

    #[test]
    fn stale_events_are_dropped() {
        let t0 = Instant::now();
        let mut l = live_link(t0);
        // A late Detached of an attachment we left: ignored.
        l.on_event(Event::Detached { attach_seq: 0, session_id: None, reason: "old".into() }, t0);
        assert!(l.live());
        for ev in turn(7) {
            l.on_event(ev, t0);
        }
        assert_eq!(l.body(t0).mood, Mood::Idle);
        assert_eq!(l.body(t0).reply, "");
    }

    #[test]
    fn reattach_is_a_full_replace_and_plays_the_appear_glitch() {
        let t0 = Instant::now();
        let mut l = live_link(t0);
        let evs = turn(1);
        for ev in &evs[..60] {
            l.on_event(ev.clone(), t0);
        }
        assert!(l.body(t0).busy);
        l.submitted("typed".into());
        l.on_event(Event::Reattaching { attach_seq: 1, session_id: "s".into(), reason: "reload".into() }, t0);
        assert_eq!(l.body(t0).mood, Mood::Away);
        assert_eq!(l.take_once(), vec![Once::Restore("typed".into())]);
        let Event::Attached { attached, .. } = attached(1, "attached_history.json") else { panic!() };
        l.on_event(Event::Attached { attach_seq: 1, attached, mode: AttachMode::Mirror, reconnect: true }, t0);
        let b = l.body(t0);
        assert!(!b.busy);
        assert_eq!(b.mood, Mood::Idle);
        // The last reply comes from the snapshot, not from the half-streamed turn.
        assert!(b.reply.ends_with("Done! The echo command returned `hi` as expected."), "{}", b.reply);
        assert_eq!(l.take_once(), vec![Once::Appear]);
    }

    #[test]
    fn detach_goes_away_and_looks_again() {
        let t0 = Instant::now();
        let mut l = live_link(t0);
        l.on_event(Event::Detached { attach_seq: 1, session_id: Some("s".into()), reason: "ended".into() }, t0);
        assert_eq!(l.body(t0).mood, Mood::Away);
        assert!(l.tick(t0).is_empty());
        let t1 = t0 + RETRY_SOON;
        assert!(matches!(l.tick(t1).as_slice(), [Act::List]));
        // The list has no `avatar` and nothing to follow: create it, by name.
        let acts = l.on_event(Event::Sessions(vec![]), t1);
        match acts.as_slice() {
            [Act::Create(o)] => assert_eq!(o.name.as_deref(), Some(DEFAULT_NAME)),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_named_create_that_fails_resumes_then_creates_unnamed() {
        let t0 = Instant::now();
        let mut l = Link::new(opts(), t0);
        l.on_event(connected(), t0);
        assert!(matches!(l.on_event(Event::Sessions(vec![]), t0).as_slice(), [Act::Create(_)]));
        l.attaching(1);
        l.on_event(Event::Detached { attach_seq: 1, session_id: None, reason: "name taken".into() }, t0);
        let t1 = t0 + RETRY_SOON;
        match l.on_event(Event::Sessions(vec![]), t1).as_slice() {
            [Act::Resume(n, _)] => assert_eq!(n, DEFAULT_NAME),
            other => panic!("{other:?}"),
        }
        l.attaching(2);
        l.on_event(Event::Detached { attach_seq: 2, session_id: None, reason: "no session".into() }, t1);
        match l.on_event(Event::Sessions(vec![]), t1 + RETRY_SOON).as_slice() {
            [Act::Create(o)] => assert_eq!(o.name, None),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn refused_input_comes_back() {
        let t0 = Instant::now();
        let mut l = live_link(t0);
        l.submitted("hello there".into());
        l.on_event(Event::Error { attach_seq: Some(1), error: Error::Refused { command: Some("submit".into()), reason: "not owner".into() } }, t0);
        assert_eq!(l.take_once(), vec![Once::Restore("hello there".into())]);
        // A turn that starts takes it: nothing to give back later.
        l.submitted("again".into());
        l.on_event(turn(1)[1].clone(), t0);
        l.on_event(Event::Detached { attach_seq: 1, session_id: None, reason: "x".into() }, t0);
        assert_eq!(l.take_once(), vec![]);
    }

    #[test]
    fn prompts_need_you_until_resolved() {
        let t0 = Instant::now();
        let mut l = live_link(t0);
        let req: PromptRequest = serde_json::from_value(serde_json::json!({"id": 4, "kind": "secret", "title": "password required", "prompt": "test password:"})).unwrap();
        let ev = |event| Event::Session { attach_seq: 1, session_id: "s".into(), seq: 0, event, raw: Arc::new(serde_json::Value::Null) };
        l.on_event(ev(SessionEvent::Prompt { request: req }), t0);
        let b = l.body(t0);
        assert_eq!(b.mood, Mood::NeedsYou);
        assert_eq!(b.prompt, Some(PromptView { id: 4, title: "password required".into(), text: "test password:".into(), secret: true }));
        l.on_event(ev(SessionEvent::PromptResolved { prompt_id: 4 }), t0);
        assert_eq!(l.body(t0).prompt, None);
        assert_eq!(l.body(t0).mood, Mood::Idle);
    }

    #[test]
    fn abort_is_annoyed_with_a_reason() {
        let t0 = Instant::now();
        let mut l = live_link(t0);
        for ev in &turn(1)[..62] {
            l.on_event(ev.clone(), t0);
        }
        let ev = |event| Event::Session { attach_seq: 1, session_id: "s".into(), seq: 0, event, raw: Arc::new(serde_json::Value::Null) };
        l.on_event(ev(SessionEvent::Aborted { context_saved: true }), t0);
        l.on_event(ev(SessionEvent::Idle), t0);
        let b = l.body(t0);
        assert_eq!(b.mood, Mood::Annoyed);
        assert_eq!(b.reply, "Here's a simple Python hello world:\n‹code›\n‹aborted›");
        assert_eq!(l.body(t0 + ANNOYED_FOR).mood, Mood::Idle);
    }

    #[test]
    fn boot_grace_then_away() {
        let t0 = Instant::now();
        let l = Link::new(opts(), t0);
        assert_eq!(l.body(t0).mood, Mood::Idle);
        assert_eq!(l.body(t0 + BOOT_GRACE).mood, Mood::Away);
    }

    #[test]
    fn markdown_lite() {
        assert_eq!(clean("# Title\n\n**bold** and `code`\n- one\n* two\n[link](http://x) end"), "Title\nbold and `code`\n• one\n• two\nlink end");
        assert_eq!(clean("a\n```rust\nfn x() {}\n```\nb"), "a\n‹code›\nb");
        assert_eq!(clean("a\n```rust\nfn x"), "a\n‹code›");
        assert_eq!(clean("#hashtag"), "#hashtag");
        assert_eq!(tool_label("mcp__github__create_pull_request"), "create_pull_request".chars().take(17).collect::<String>() + "…");
        assert_eq!(tool_label("bash"), "bash");
        assert_eq!(one_line("  a \n b  ", 10), "a b");
    }
}
