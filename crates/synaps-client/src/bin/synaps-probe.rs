//! Headless probe: proves the client layer without a display.
//!
//! Connects, lists sessions, creates (or attaches to) a session, submits a
//! short prompt, streams the reply to stdout and prints timings as JSON.
//!
//! ```text
//! synaps-probe --profile desktopproto [--prompt TEXT] [--model M] [--session ID]
//!              [--socket PATH] [--list] [--stay SECS]
//! ```
//! `--stay` keeps the session attached after the turn (to exercise a
//! `synaps daemon reload` reconnect by hand).

use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use synaps_client::protocol::{LlmEvent, SessionEvent, StreamEvent};
use synaps_client::transcript::Transcript;
use synaps_client::{AttachMode, Client, ClientOptions, CreateOptions, Endpoint, Event, Identity, Status};

fn main() {
    let t0 = Instant::now();
    let mut profile: Option<String> = None;
    let mut socket: Option<PathBuf> = None;
    let mut prompt = "Reply with exactly: hello from the desktop probe".to_string();
    let mut model: Option<String> = None;
    let mut session: Option<String> = None;
    let mut list_only = false;
    let mut stay = 0u64;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--profile" => profile = args.next(),
            "--socket" => socket = args.next().map(PathBuf::from),
            "--prompt" => prompt = args.next().unwrap_or_default(),
            "--model" => model = args.next(),
            "--session" => session = args.next(),
            "--list" => list_only = true,
            "--stay" => stay = args.next().and_then(|s| s.parse().ok()).unwrap_or(0),
            "-h" | "--help" => {
                println!("synaps-probe --profile P [--prompt T] [--model M] [--session ID] [--socket PATH] [--list] [--stay SECS]");
                return;
            }
            other => {
                eprintln!("unknown argument {other}");
                std::process::exit(2);
            }
        }
    }
    let endpoint = match socket {
        Some(s) => Endpoint::Socket(s),
        None => Endpoint::profile(profile.as_deref()),
    };
    eprintln!("probe: socket {}", endpoint.resolve().display());
    let identity = Identity::new("probe").expect("valid name").with_version(format!("synaps-probe/{}", env!("CARGO_PKG_VERSION")));
    let (client, events) = match Client::spawn(ClientOptions::new(endpoint, identity)) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("probe: {e}");
            std::process::exit(1);
        }
    };

    let mut connected_ms = None;
    let deadline = Instant::now() + Duration::from_secs(10);
    // Wait for the control connection and the first session list.
    let sessions = loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match recv(&events, left) {
            Some(Event::Status(Status::Connected { daemon_version, profile, pid, .. })) => {
                connected_ms.get_or_insert(t0.elapsed().as_secs_f64() * 1e3);
                eprintln!("probe: connected to daemon {daemon_version} (profile {profile:?}, pid {pid})");
            }
            Some(Event::Status(Status::Incompatible(e))) => {
                eprintln!("probe: incompatible: {e}");
                std::process::exit(1);
            }
            Some(Event::Status(Status::Offline(e))) => {
                eprintln!("probe: offline: {e}");
                std::process::exit(1);
            }
            Some(Event::Sessions(s)) => break s,
            Some(_) => {}
            None => {
                eprintln!("probe: timed out connecting");
                std::process::exit(1);
            }
        }
    };
    eprintln!("probe: {} session(s)", sessions.len());
    for s in &sessions {
        eprintln!("  {}  {:<8} busy={} msgs={} {}", s.id, s.lifecycle, s.busy, s.messages, s.label());
    }
    if list_only {
        return;
    }

    let attach_start = Instant::now();
    let mut create = CreateOptions::new();
    if let Some(m) = model {
        create = create.model(m);
    }
    let seq = match session {
        Some(id) => client.attach(id, AttachMode::Mirror),
        None => client.create(create),
    }
    .expect("client thread");
    let mut transcript = Transcript::new();
    let session_id = loop {
        match recv(&events, Duration::from_secs(30)) {
            Some(Event::Attached { attached, attach_seq, .. }) if attach_seq == seq => {
                if let Err(e) = transcript.load_attached(&attached) {
                    eprintln!("probe: {e}");
                    std::process::exit(1);
                }
                eprintln!(
                    "probe: attached {} ({} history messages -> {} rows) in {:.1} ms",
                    attached.meta.id,
                    attached.conversation.api_messages.len(),
                    transcript.len(),
                    attach_start.elapsed().as_secs_f64() * 1e3
                );
                break attached.meta.id.clone();
            }
            Some(Event::Detached { reason, .. }) => {
                eprintln!("probe: attach failed: {reason}");
                std::process::exit(1);
            }
            Some(_) => {}
            None => {
                eprintln!("probe: timed out attaching");
                std::process::exit(1);
            }
        }
    };
    let attach_ms = attach_start.elapsed().as_secs_f64() * 1e3;

    let submit = Instant::now();
    client.submit(prompt).expect("client thread");
    let (mut first_delta, mut first_text) = (None, None);
    let mut deltas = 0usize;
    let mut started = false;
    let mut out = std::io::stdout().lock();
    let done = loop {
        match recv(&events, Duration::from_secs(120)) {
            Some(Event::Session { event, .. }) => {
                if let SessionEvent::Stream { event: StreamEvent::Llm(l) } = &event {
                    match l {
                        LlmEvent::Text { text } => {
                            first_text.get_or_insert(submit.elapsed());
                            first_delta.get_or_insert(submit.elapsed());
                            deltas += 1;
                            let _ = out.write_all(text.as_bytes());
                            let _ = out.flush();
                        }
                        LlmEvent::Thinking { .. } => {
                            first_delta.get_or_insert(submit.elapsed());
                            deltas += 1;
                        }
                        _ => {}
                    }
                }
                if matches!(event, SessionEvent::TurnStarted { .. }) {
                    started = true;
                }
                let idle = matches!(event, SessionEvent::Idle);
                transcript.apply(&event);
                if idle && started {
                    break submit.elapsed();
                }
            }
            Some(Event::History { update, .. }) => {
                if let Err(e) = transcript.apply_update(&update) {
                    eprintln!("\nprobe: {e}");
                    let _ = client.resync();
                }
            }
            Some(Event::Error { error, .. }) => eprintln!("\nprobe: error: {error}"),
            Some(Event::Detached { reason, .. }) => {
                eprintln!("\nprobe: detached: {reason}");
                std::process::exit(1);
            }
            Some(_) => {}
            None => {
                eprintln!("\nprobe: timed out waiting for the turn");
                std::process::exit(1);
            }
        }
    };
    let _ = writeln!(out);
    drop(out);

    if stay > 0 {
        eprintln!("probe: staying attached {stay}s (try `synaps daemon reload --profile ...`)");
        let until = Instant::now() + Duration::from_secs(stay);
        while let Some(ev) = recv(&events, until.saturating_duration_since(Instant::now())) {
            match ev {
                Event::Status(s) => eprintln!("probe: status {s:?}"),
                Event::Attached { attached, reconnect, .. } => eprintln!(
                    "probe: attached {} again (reconnect={reconnect}, {} messages)",
                    attached.meta.id,
                    attached.conversation.api_messages.len()
                ),
                Event::Session { event, .. } => eprintln!("probe: event {}", format!("{event:?}").chars().take(100).collect::<String>()),
                Event::Detached { reason, .. } => {
                    eprintln!("probe: detached: {reason}");
                    break;
                }
                _ => {}
            }
        }
    }
    let _ = client.shutdown();
    std::thread::sleep(Duration::from_millis(300));
    let ms = |d: Option<Duration>| d.map(|d| (d.as_secs_f64() * 1e4).round() / 10.0);
    println!(
        "{}",
        serde_json::json!({
            "session_id": session_id,
            "connect_ms": connected_ms,
            "attach_ms": (attach_ms * 10.0).round() / 10.0,
            "ttft_ms": ms(first_text),
            "first_delta_ms": ms(first_delta),
            "turn_ms": ms(Some(done)),
            "stream_deltas": deltas,
            "transcript_rows": transcript.len(),
        })
    );
}

fn recv(rx: &async_channel::Receiver<Event>, timeout: Duration) -> Option<Event> {
    let until = Instant::now() + timeout;
    loop {
        match rx.try_recv() {
            Ok(e) => return Some(e),
            Err(async_channel::TryRecvError::Closed) => return None,
            Err(async_channel::TryRecvError::Empty) => {
                if Instant::now() >= until {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }
}
