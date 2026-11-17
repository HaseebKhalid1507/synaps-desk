//! `--bench`: self-driving measurements, printed as one JSON object, then
//! exit. Synthetic load comes from `synaps_client::synthetic` and flows
//! through the same event path as daemon traffic (channel → pump → model,
//! then the transcript's splice once per frame). Frame data comes from GPUI's own profiler
//! (`FrameEvent::Draw` / `Present`).

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui_kit::profiler::{FrameEvent, FrameTimingCollector};
use gpui_kit::*;
use serde_json::{Value, json};
use std::sync::Arc;
use synaps_client::protocol::{LlmEvent, SessionEvent, SessionStreamEvent, StreamEvent};

use synaps_client::protocol::{DaemonFrame, SessionMeta, decode_frame};
use synaps_client::{AttachMode, Event, synthetic};

use crate::app::{AppView, Open};
use crate::mem;
use crate::model::LOCAL_SEQ;

pub struct BenchOptions {
    pub t_main: Instant,
    pub exec_to_main_ms: Option<f64>,
    pub refresh_hz: Option<f64>,
    pub prompt: Option<String>,
    pub stream_secs: f64,
    pub scroll_secs: f64,
    pub scroll_px_per_s: f32,
    pub drag_secs: f64,
    pub history_messages: usize,
    pub diag: bool,
    /// `--bench-tabs`: only the tab measurements ([`TAB_SESSIONS`] sessions
    /// of [`DRAG_TURNS`] turns: RSS with 1, 4 and 8 tabs, switch times
    /// cached and not).
    pub tabs: bool,
    /// `--bench-history FILE`: the tab bench's sessions all have this
    /// history (a saved session's `api_messages`: a real one, with its
    /// tool results and edits), not the synthetic one.
    pub history_file: Option<std::path::PathBuf>,
    /// `--bench-motion`: what motion costs (motion.md §9): presents over
    /// 10 s idle, then a running tool's heartbeat (frames, phases, CPU per
    /// frame), then the same with Motion off.
    pub motion: bool,
}

/// CPU time of every thread of this process so far (`/proc/self/task/*/schedstat`,
/// nanoseconds; Linux only, else 0).
fn cpu_ns() -> u64 {
    let Ok(tasks) = std::fs::read_dir("/proc/self/task") else { return 0 };
    tasks
        .flatten()
        .filter_map(|t| std::fs::read_to_string(t.path().join("schedstat")).ok())
        .filter_map(|s| s.split_whitespace().next()?.parse::<u64>().ok())
        .sum()
}

pub struct BenchState {
    collector: FrameTimingCollector,
    pub frames: Vec<FrameEvent>,
    pub scroll: Option<(Instant, f32)>,
    /// A side panel's edge dragged back and forth across its limits
    /// (`rail`, else the right panel), one new width a frame.
    pub drag: Option<(Instant, bool)>,
    pub animate: bool,
    pub submit_at: Option<Instant>,
    turn_seen: bool,
    pub first_text_at: Option<Instant>,
    pub idle_at: Option<Instant>,
    pub attached_at: Option<Instant>,
    /// When the last `Attached` reached the pump (the tab bench).
    pub last_attached: Option<Instant>,
    pub received_deltas: usize,
}

impl BenchState {
    pub fn new() -> Self {
        Self {
            collector: FrameTimingCollector::new(),
            frames: Vec::new(),
            scroll: None,
            drag: None,
            animate: false,
            submit_at: None,
            turn_seen: false,
            first_text_at: None,
            idle_at: None,
            attached_at: None,
            last_attached: None,
            received_deltas: 0,
        }
    }

    pub fn on_received(&mut self, batch: &[Event], now: Instant) {
        for e in batch {
            match e {
                Event::Attached { .. } => {
                    self.attached_at.get_or_insert(now);
                    self.last_attached = Some(now);
                }
                Event::Session { event, .. } => match event {
                    SessionEvent::Stream { event: StreamEvent::Llm(LlmEvent::Text { .. }) } => {
                        self.received_deltas += 1;
                        if self.submit_at.is_some() {
                            self.first_text_at.get_or_insert(now);
                        }
                    }
                    SessionEvent::TurnStarted { .. } if self.submit_at.is_some() => self.turn_seen = true,
                    SessionEvent::Idle if self.turn_seen => {
                        self.idle_at.get_or_insert(now);
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }

    pub fn on_render(&mut self, list: &ListState, window: &mut Window) {
        self.frames.extend(self.collector.collect_unseen());
        if let Some((last, speed)) = self.scroll.as_mut() {
            let now = Instant::now();
            let dt = now.duration_since(*last).as_secs_f32();
            *last = now;
            list.scroll_by(px(dt * *speed));
        }
        if self.animate || self.scroll.is_some() {
            window.request_animation_frame();
        }
    }

    /// The width the dragged edge is at now: [`DRAG_PX_PER_S`] back and
    /// forth between the panel's limits.
    pub fn drag_width(&self) -> Option<(bool, f32)> {
        let (t0, rail) = self.drag?;
        let (lo, hi) = if rail { crate::theme::RAIL_WIDTHS } else { crate::theme::RIGHT_WIDTHS };
        let (lo, hi) = (f32::from(lo), f32::from(hi));
        let x = (t0.elapsed().as_secs_f32() * DRAG_PX_PER_S) % (2. * (hi - lo));
        Some((rail, if x < hi - lo { lo + x } else { hi - (x - (hi - lo)) }))
    }

    fn drain(&mut self) {
        self.frames.extend(self.collector.collect_unseen());
    }

    fn first_present_after(&self, t: Instant) -> Option<Instant> {
        self.frames.iter().find_map(|f| match f {
            FrameEvent::Present(p) if p.present_end >= t => Some(p.present_end),
            _ => None,
        })
    }

    /// The first frame drawn from `t` on: its draw time and when it was
    /// on screen.
    fn frame_after(&self, t: Instant) -> Option<(Duration, Instant)> {
        let (k, draw) = self.frames.iter().enumerate().find_map(|(k, f)| match f {
            FrameEvent::Draw(d) if d.draw_start >= t => Some((k, d.draw_duration())),
            _ => None,
        })?;
        let shown = self.frames[k..].iter().find_map(|f| match f {
            FrameEvent::Present(p) => Some(p.present_end),
            _ => None,
        })?;
        Some((draw, shown))
    }

    fn refresh(&self) -> Option<Duration> {
        self.frames.iter().find_map(|f| match f {
            FrameEvent::Draw(d) => d.refresh_interval,
            _ => None,
        })
    }

    /// Frame statistics for `[t0, t1)`.
    fn stats(&self, t0: Instant, t1: Instant, refresh_fallback: Option<Duration>) -> Value {
        let mut draws: Vec<f64> = Vec::new();
        let mut presents: Vec<Instant> = Vec::new();
        let mut late = 0usize;
        let (mut layout, mut prepaint, mut paint, mut present) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let refresh_source = if self.refresh().is_some() {
            "gpui"
        } else if refresh_fallback.is_some() {
            "--refresh-hz"
        } else {
            "unknown"
        };
        let refresh = self.refresh().or(refresh_fallback);
        for f in &self.frames {
            match f {
                FrameEvent::Draw(d) if d.draw_start >= t0 && d.draw_start < t1 => {
                    draws.push(ms(d.draw_duration()));
                    layout.push(ms(d.phases.request_layout));
                    prepaint.push(ms(d.phases.prepaint));
                    paint.push(ms(d.phases.paint));
                    if let Some(r) = refresh
                        && d.draw_duration() > r
                    {
                        late += 1;
                    }
                }
                FrameEvent::Present(p) if p.present_end >= t0 && p.present_end < t1 => {
                    presents.push(p.present_end);
                    present.push(ms(p.present_duration()));
                }
                _ => {}
            }
        }
        let intervals: Vec<f64> = presents.windows(2).map(|w| ms(w[1].duration_since(w[0]))).collect();
        let dropped: Option<u64> = refresh.map(|r| {
            let r = ms(r);
            intervals.iter().map(|&i| ((i / r).round() as u64).saturating_sub(1)).sum()
        });
        let secs = t1.duration_since(t0).as_secs_f64();
        let expected = refresh.map(|r| (secs / r.as_secs_f64()).floor() as u64);
        json!({
            "duration_s": round(secs, 2),
            "frames_presented": presents.len(),
            "fps": round(presents.len() as f64 / secs, 1),
            "refresh_hz": refresh.map(|r| round(1.0 / r.as_secs_f64(), 2)),
            "refresh_source": refresh_source,
            "refreshes_in_window": expected,
            "dropped_frames": dropped,
            "draw_ms": pct(&draws),
            "draw_phases_p50_ms": {"request_layout": pct(&layout)["p50"], "prepaint": pct(&prepaint)["p50"], "paint": pct(&paint)["p50"]},
            "draw_phases_p95_ms": {"request_layout": pct(&layout)["p95"], "prepaint": pct(&prepaint)["p95"], "paint": pct(&paint)["p95"]},
            "present_submit_ms": pct(&present),
            "frame_interval_ms": pct(&intervals),
            "draws_over_budget": late,
        })
    }
}

/// How fast the bench drags an edge.
const DRAG_PX_PER_S: f32 = 400.;
/// The drag scenario's transcript, in user turns.
const DRAG_TURNS: usize = 400;
/// The tab bench's sessions.
const TAB_SESSIONS: usize = 8;
/// Its tab switches, cached and not (each half).
const TAB_SWITCHES: usize = 8;

/// A synthetic history of `n` user turns.
fn turns(n: usize, seed: u64) -> Vec<Value> {
    let mut h = dated(synthetic::history(n * 3, seed));
    let cut = h.iter().enumerate().filter(|(_, m)| m["role"] == "user" && m["content"].is_string()).nth(n).map(|(i, _)| i);
    h.truncate(cut.unwrap_or(h.len()));
    h
}

/// A daemon's `Attached` for `id` with history `h`, through the client's
/// own decoder.
fn attached_event(id: &str, attach_seq: u64, h: &[Value]) -> Option<Event> {
    let frame = json!({"type": "attached", "client": 2, "input_owner": 2, "client_input": "shared",
        "meta": {"id": id, "model": "m", "lifecycle": "live"},
        "conversation": {"messages_len": h.len(), "api_messages": h}});
    match decode_frame(&frame.to_string()) {
        Ok(DaemonFrame::Attached(attached)) => Some(Event::Attached { attach_seq, attached, mode: AttachMode::Mirror, reconnect: false }),
        _ => None,
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

fn round(v: f64, p: i32) -> f64 {
    let m = 10f64.powi(p);
    (v * m).round() / m
}

fn pct(v: &[f64]) -> Value {
    if v.is_empty() {
        return Value::Null;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.total_cmp(b));
    let q = |p: f64| round(s[((s.len() - 1) as f64 * p).round() as usize], 2);
    json!({"n": s.len(), "p50": q(0.5), "p95": q(0.95), "p99": q(0.99), "max": q(1.0),
        "mean": round(s.iter().sum::<f64>() / s.len() as f64, 2)})
}

fn epoch_ms(at: Instant) -> f64 {
    let now = Instant::now();
    let wall = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs_f64() * 1e3;
    wall - ms(now.saturating_duration_since(at))
}

/// The bench script. Runs on the UI thread between frames.
pub fn start(view: &Entity<AppView>, opts: BenchOptions, window: &mut Window, cx: &mut App) {
    // Hard stop: never leave a window up if anything hangs.
    let limit = if opts.motion { 200 } else { 30 };
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(limit));
        eprintln!("bench: watchdog fired at {limit} s");
        std::process::exit(3);
    });
    let (tx, rx) = async_channel::unbounded::<Event>();
    view.update(cx, |v, cx| {
        v.bench = Some(BenchState::new());
        v.pump(rx, cx);
    });
    let view = view.downgrade();
    window
        .spawn(cx, async move |cx| {
            let t_main = opts.t_main;
            let refresh_fallback = opts.refresh_hz.map(|hz| Duration::from_secs_f64(1.0 / hz));
            let ex = cx.background_executor().clone();
            let sleep = |d: f64| ex.timer(Duration::from_secs_f64(d));
            let upd = |cx: &mut AsyncWindowContext, f: &dyn Fn(&mut AppView, &mut Context<AppView>)| {
                view.update(cx, |v, cx| {
                    f(v, cx);
                    cx.notify();
                })
            };
            let mut out = json!({"bench": "synaps-desktop", "version": env!("CARGO_PKG_VERSION"),
                "exec_to_main_ms": opts.exec_to_main_ms});

            // 1. First frame.
            let mut first = None;
            for _ in 0..300 {
                sleep(0.01).await;
                first = view
                    .update(cx, |v, _| {
                        let b = v.bench.as_mut().unwrap();
                        b.drain();
                        b.first_present_after(t_main)
                    })
                    .ok()
                    .flatten();
                if first.is_some() {
                    break;
                }
            }
            if let Some(f) = first {
                out["main_to_first_frame_ms"] = json!(round(ms(f.duration_since(t_main)), 2));
                out["first_frame_epoch_ms"] = json!(round(epoch_ms(f), 1));
                out["exec_to_first_frame_ms_approx"] = json!(opts.exec_to_main_ms.map(|e| round(e + ms(f.duration_since(t_main)), 1)));
            }
            sleep(1.5).await;
            out["mem_idle"] = mem::sample();
            if opts.tabs {
                out["mem_idle"]["trimmed_rss_kb"] = mem::trimmed_rss_kb();
                // A synthetic daemon with 8 sessions of 400 turns; it answers
                // each attach off the UI thread (encode, decode), as the
                // connection thread would.
                let real = opts.history_file.as_ref().map(|p| {
                    let v: Value = std::fs::read(p).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
                    let h = v.get("api_messages").or(Some(&v)).and_then(Value::as_array).cloned().unwrap_or_default();
                    (p.metadata().map(|m| m.len()).unwrap_or(0), Arc::new(h))
                });
                out["history_file"] = json!(real.as_ref().map(|(bytes, h)| json!({"bytes": bytes, "messages": h.len()})));
                let hist: Arc<Vec<Arc<Vec<Value>>>> = Arc::new(
                    (0..TAB_SESSIONS)
                        .map(|k| match &real {
                            Some((_, h)) => h.clone(),
                            None => Arc::new(turns(DRAG_TURNS, 30 + k as u64)),
                        })
                        .collect(),
                );
                let ids: Arc<Vec<String>> = Arc::new((0..TAB_SESSIONS).map(|k| format!("20261114-0900{k:02}-b{k:03}")).collect());
                let metas: Vec<SessionMeta> = ids
                    .iter()
                    .enumerate()
                    .filter_map(|(k, id)| {
                        serde_json::from_value(json!({"id": id, "title": format!("Bench tab {}", k + 1), "model": "m", "lifecycle": "live",
                            "messages": hist[k].len(), "created_at": format!("2026-11-14T09:00:{k:02}Z")}))
                        .ok()
                    })
                    .collect();
                let _ = tx.try_send(Event::Sessions(metas));
                // One thread answers every attach, as the client's one
                // connection thread decodes them (a thread each would give
                // the allocator an arena each, and the RSS with it).
                let (req_tx, req_rx) = std::sync::mpsc::channel::<(String, u64)>();
                let (tx_conn, ids_conn, hist_conn) = (tx.clone(), ids.clone(), hist.clone());
                std::thread::spawn(move || {
                    for (sid, seq) in req_rx {
                        if let Some(k) = ids_conn.iter().position(|i| *i == sid)
                            && let Some(ev) = attached_event(&sid, seq, &hist_conn[k])
                        {
                            let _ = tx_conn.try_send(ev);
                        }
                    }
                });
                let tx2 = tx.clone();
                let _ = upd(cx, &move |v, _| {
                    v.synthetic_tx = Some(tx2.clone());
                    let req = req_tx.clone();
                    v.attach_source = Some(Box::new(move |sid: &str, seq: u64| {
                        let _ = req.send((sid.to_string(), seq));
                        Vec::new()
                    }));
                });
                sleep(0.3).await;
                let wait_attached = async |cx: &mut AsyncWindowContext, id: &str| {
                    for _ in 0..400 {
                        let done = view.update(cx, |v, _| v.model.is_active(id)).unwrap_or(true);
                        if done {
                            return Some(Instant::now());
                        }
                        sleep(0.005).await;
                    }
                    None
                };
                // The first frame drawn from `t` on, once it was on screen.
                let wait_frame = async |cx: &mut AsyncWindowContext, t: Instant| {
                    for _ in 0..400 {
                        let f = view
                            .update(cx, |v, _| {
                                let b = v.bench.as_mut().unwrap();
                                b.drain();
                                b.frame_after(t)
                            })
                            .ok()
                            .flatten();
                        if f.is_some() {
                            return f;
                        }
                        sleep(0.002).await;
                    }
                    None
                };
                let mut mem_tabs = json!({});
                for (k, id) in ids.iter().enumerate() {
                    let id2 = id.clone();
                    let _ = upd(cx, &move |v, _| {
                        let place = if k == 0 { crate::tabs::Place::Current } else { crate::tabs::Place::New };
                        v.open_tab(Open::Attach { session_id: id2.clone(), fallback: None, open: false }, place);
                    });
                    if let Some(t) = wait_attached(cx, id).await {
                        wait_frame(cx, t).await;
                    }
                    if matches!(k + 1, 1 | 4 | 8) {
                        sleep(0.5).await;
                        let mut m = mem::sample();
                        m["trimmed_rss_kb"] = mem::trimmed_rss_kb();
                        let (n, bytes) = view.update(cx, |v, _| (v.tab_cache.len(), v.tab_cache.bytes())).unwrap_or((0, 0));
                        m["cached_tabs"] = json!(n);
                        m["cache_estimate_mib"] = json!(round(bytes as f64 / 1048576.0, 2));
                        mem_tabs[format!("tabs_{}", k + 1)] = m;
                    }
                }
                out["mem_tabs"] = mem_tabs;
                let rows = view.update(cx, |v, _| v.model.transcript.len()).unwrap_or(0);
                // Switch between the first two tabs; the second half drops the
                // target's cache first (a tab that fell out of it).
                let mut runs: Vec<Value> = Vec::new();
                for k in 0..2 * TAB_SWITCHES {
                    let target = k % 2;
                    let uncached = k >= TAB_SWITCHES;
                    let id = ids[target].clone();
                    sleep(0.25).await;
                    let t0 = Instant::now();
                    let id2 = id.clone();
                    let _ = upd(cx, &move |v, cx| {
                        if uncached {
                            let _ = v.tab_cache.take(&id2);
                        }
                        // A click (a key would wait for its key-up).
                        v.focus_tab(target);
                        cx.notify();
                    });
                    let switched = Instant::now();
                    // What the switch put on screen before any attach: the
                    // cached rows, or none.
                    let rows_at_switch = view.update(cx, |v, _| v.model.transcript.len()).unwrap_or(0);
                    let shown = wait_frame(cx, t0).await;
                    wait_attached(cx, &id).await;
                    let at = view.update(cx, |v, _| v.bench.as_ref().unwrap().last_attached).ok().flatten().filter(|a| *a >= t0);
                    let drawn = match at {
                        Some(t) => wait_frame(cx, t).await,
                        None => None,
                    };
                    let since = |t: Instant| round(ms(t.duration_since(t0)), 2);
                    runs.push(json!({
                        "uncached": uncached,
                        "switch_call_ms": since(switched),
                        "rows_at_switch": rows_at_switch,
                        "first_frame_draw_ms": shown.map(|f| round(ms(f.0), 2)),
                        "first_frame_shown_ms": shown.map(|f| since(f.1)),
                        "attached_ms": at.map(since),
                        "attached_frame_draw_ms": drawn.map(|f| round(ms(f.0), 2)),
                        "attached_frame_shown_ms": drawn.map(|f| since(f.1)),
                    }));
                }
                let keys = [
                    "rows_at_switch",
                    "switch_call_ms",
                    "first_frame_draw_ms",
                    "first_frame_shown_ms",
                    "attached_ms",
                    "attached_frame_draw_ms",
                    "attached_frame_shown_ms",
                ];
                let summary = |uncached: bool, key: &str| {
                    let v: Vec<f64> = runs.iter().filter(|r| r["uncached"] == uncached).filter_map(|r| r[key].as_f64()).collect();
                    pct(&v)
                };
                out["tab_switch"] = json!({
                    "turns": DRAG_TURNS, "rows": rows, "messages": hist[0].len(),
                    "cached": keys.iter().map(|k| (k.to_string(), summary(false, k))).collect::<serde_json::Map<_, _>>(),
                    "uncached": keys.iter().map(|k| (k.to_string(), summary(true, k))).collect::<serde_json::Map<_, _>>(),
                    "runs": runs,
                });
                out["mem_end"] = mem::sample();
                out["total_s"] = json!(round(t_main.elapsed().as_secs_f64(), 2));
                println!("{out}");
                let _ = cx.update(|_, cx| cx.quit());
                return;
            }
            if opts.motion {
                // The app's own frames and the beat layer's (a beat that
                // repaints only the layer is not an app frame).
                let counts =
                    |cx: &mut AsyncWindowContext| view.update(cx, |v, _| (v.fx.frames(), v.fx.layer_paints.get())).unwrap_or((0, 0));
                let stats = |cx: &mut AsyncWindowContext, t0: Instant, t1: Instant, c0: u64, f0: (u64, u64)| {
                    let cpu = cpu_ns().saturating_sub(c0) as f64 / 1e6;
                    let f1 = counts(cx);
                    let mut v = view
                        .update(cx, |v, _| {
                            let b = v.bench.as_mut().unwrap();
                            b.drain();
                            b.stats(t0, t1, refresh_fallback)
                        })
                        .unwrap_or(Value::Null);
                    let secs = t1.duration_since(t0).as_secs_f64();
                    let n = v["frames_presented"].as_u64().unwrap_or(0);
                    v["cpu_ms_per_s"] = json!(round(cpu / secs, 2));
                    v["cpu_ms_per_frame"] = json!((n > 0).then(|| round(cpu / n as f64, 2)));
                    v["app_frames"] = json!(f1.0 - f0.0);
                    v["layer_paints"] = json!(f1.1 - f0.1);
                    v
                };
                let level = |cx: &mut AsyncWindowContext, l: crate::motion::Level| {
                    let _ = cx.update(|_, cx| {
                        cx.set_global(crate::motion::Setting { level: Some(l), lock: None });
                        crate::motion::Level::apply(cx);
                    });
                };
                level(cx, crate::motion::Level::Full);
                // A session at rest: nothing runs. (The synthetic history
                // ends on a call with no result, which reads as running:
                // it is the beat's scene, not this one.)
                // `SYNAPS_BENCH_IDLE_SECS=N`: N s instead, over a finished
                // history (its tool rows and prose at rest: the finish
                // gate's idle with the transcript's rows drawn).
                let idle = std::env::var("SYNAPS_BENCH_IDLE_SECS").ok().and_then(|s| s.parse::<u64>().ok());
                let _ = upd(cx, &|v, _| {
                    v.model.set_local_session("bench-motion-idle");
                    if idle.is_some() {
                        v.model.transcript.apply_history(&dated(synthetic::history(40, 3)));
                        v.sync_list();
                        v.list.scroll_to_end();
                    }
                });
                sleep(1.0).await;
                let (t0, c0, f0) = (Instant::now(), cpu_ns(), counts(cx));
                sleep(idle.unwrap_or(10) as f64).await;
                out[format!("idle_{}s", idle.unwrap_or(10))] = stats(cx, t0, Instant::now(), c0, f0);
                // A tool runs: its glyph scans on the beat (8 Hz), the foot's too.
                let _ = upd(cx, &|v, _| {
                    v.model.set_local_session("bench-motion");
                    v.model.transcript.apply_history(&dated(synthetic::history(40, 3)));
                    v.sync_list();
                    v.list.set_follow_mode(FollowMode::Tail);
                    v.list.scroll_to_end();
                    let t = &mut v.model.transcript;
                    t.apply(&SessionEvent::TurnStarted {
                        turn_baseline: 40,
                        trigger: "user".into(),
                        user_text: Some("Run the tests.".into()),
                    });
                    t.apply(&SessionEvent::Stream { event: StreamEvent::Llm(LlmEvent::ResponseStart) });
                    t.apply(&SessionEvent::Stream {
                        event: StreamEvent::Llm(LlmEvent::ToolUseStart { tool_name: "bash".into(), tool_id: "bench-run".into() }),
                    });
                    t.apply(&SessionEvent::Stream {
                        event: StreamEvent::Llm(LlmEvent::ToolUse {
                            tool_name: "bash".into(),
                            tool_id: "bench-run".into(),
                            input: json!({"command": "cargo test --workspace"}),
                        }),
                    });
                    v.sync_list();
                    v.list.scroll_to_end();
                });
                sleep(1.0).await;
                let (t0, c0, f0) = (Instant::now(), cpu_ns(), counts(cx));
                sleep(6.0).await;
                out["beat"] = stats(cx, t0, Instant::now(), c0, f0);
                // 24 more: 25 glyphs scan on the one beat.
                let _ = upd(cx, &|v, _| {
                    let t = &mut v.model.transcript;
                    for k in 0..24 {
                        let id = format!("bench-run-{k}");
                        t.apply(&SessionEvent::Stream {
                            event: StreamEvent::Llm(LlmEvent::ToolUseStart { tool_name: "bash".into(), tool_id: id.clone() }),
                        });
                        t.apply(&SessionEvent::Stream {
                            event: StreamEvent::Llm(LlmEvent::ToolUse {
                                tool_name: "bash".into(),
                                tool_id: id,
                                input: json!({"command": format!("cargo test -p crate{k}")}),
                            }),
                        });
                    }
                    v.sync_list();
                    v.list.scroll_to_end();
                });
                sleep(1.0).await;
                let (t0, c0, f0) = (Instant::now(), cpu_ns(), counts(cx));
                out["beat_25_sites"] = json!(view.update(cx, |v, _| v.fx.site_count()).unwrap_or(0));
                sleep(30.0).await;
                out["beat_25_30s"] = stats(cx, t0, Instant::now(), c0, f0);
                level(cx, crate::motion::Level::Off);
                let _ = upd(cx, &|_, _| {});
                sleep(0.5).await;
                let (t0, c0, f0) = (Instant::now(), cpu_ns(), counts(cx));
                sleep(4.0).await;
                out["beat_motion_off"] = stats(cx, t0, Instant::now(), c0, f0);
                // Q4's cases after the beat's (those as main runs them): a
                // fresh transcript, motion Full again, nothing running.
                level(cx, crate::motion::Level::Full);
                let _ = upd(cx, &|v, _| {
                    v.model.transcript = synaps_client::transcript::Transcript::new();
                    v.sync_list();
                });
                sleep(0.5).await;
                let (t0, c0, f0) = (Instant::now(), cpu_ns(), counts(cx));
                sleep(10.0).await;
                out["idle_10s_after_beat"] = stats(cx, t0, Instant::now(), c0, f0);
                // polish.md §6 gate 3: 30 s with a prompt pending, after its
                // three breaths, and a demoted worker (a re-attach's
                // "started", drawn still until a live source says again).
                let _ = upd(cx, &|v, cx| {
                    v.model.set_local_session("bench");
                    let res = json!({"handle_id": "sa_1", "agent_name": "scout", "status": "running"}).to_string();
                    let worker = [
                        json!({"ev": "stream", "event": {"kind": "llm", "llm": "tool_use", "tool_name": "subagent_start", "tool_id": "w1",
                            "input": {"agent": "scout", "task": "Map the relay's upstream calls"}}}),
                        json!({"ev": "stream", "event": {"kind": "llm", "llm": "tool_result", "tool_id": "w1", "result": res}}),
                        json!({"ev": "subagent_rows", "rows": [{"subagent_id": 1, "agent_name": "scout", "status": "running",
                            "cancel_requested": false, "elapsed_secs": 3.0, "finished_elapsed": null, "step": "ls", "tools": 1}]}),
                    ];
                    v.apply_batch(worker.into_iter().filter_map(local_raw).collect(), cx);
                    v.model.stats.subagents.reattached();
                    if let Ok(request) = serde_json::from_value(json!({
                        "id": 7, "kind": "confirm", "title": "Run a command?", "prompt": "Tool: bash\nInput that will run:\n$ cargo test"
                    })) {
                        v.model.transcript.apply(&SessionEvent::Prompt { request });
                    }
                    v.sync_list();
                });
                sleep(4.0).await;
                let (t0, c0, f0) = (Instant::now(), cpu_ns(), counts(cx));
                sleep(30.0).await;
                out["idle_30s_prompt"] = stats(cx, t0, Instant::now(), c0, f0);
                out["idle_30s_prompt"]["demoted_worker"] =
                    json!(view.update(cx, |v, _| v.model.stats.subagents.demoted(1)).unwrap_or(false));
                out["idle_30s_prompt"]["prompt_pending"] =
                    json!(view.update(cx, |v, _| !v.model.transcript.pending_prompts().is_empty()).unwrap_or(false));
                // The link down with the prompt and the demoted worker still up:
                // past its grace and its drain, held (only the connection
                // word's dots step, on the beat at 2 Hz).
                let _ = upd(cx, &|v, _| {
                    v.synthetic = false;
                    v.model.conn = crate::model::ConnState::Connected("bench".into());
                });
                sleep(1.0).await;
                let _ = upd(cx, &|v, _| v.model.conn = crate::model::ConnState::Reconnecting);
                sleep(2.0).await;
                let (t0, c0, f0) = (Instant::now(), cpu_ns(), counts(cx));
                sleep(10.0).await;
                out["held_drain_10s"] = stats(cx, t0, Instant::now(), c0, f0);
                let _ = upd(cx, &|v, _| {
                    v.model.conn = crate::model::ConnState::Off("offline".into());
                    v.synthetic = true;
                });
                sleep(1.0).await;
                // Q4's own motion, each on its own: a hover sweep down the
                // rail (the pointer over every row for 2 s, one move a
                // frame), a theme cross-fade (dark to dark, `slow`).
                // A rail of twelve sessions at rest to sweep.
                let _ = upd(cx, &|v, cx| {
                    let metas: Vec<SessionMeta> = (0..12)
                        .filter_map(|k| {
                            serde_json::from_value(json!({"id": format!("20261117-0800{k:02}-h{k:03}"), "title": format!("Rest {}", k + 1),
                                "model": "m", "lifecycle": "live", "messages": 12, "created_at": format!("2026-11-17T08:00:{k:02}Z")}))
                            .ok()
                        })
                        .collect();
                    v.apply_batch(vec![Event::Sessions(metas)], cx);
                });
                sleep(1.0).await;
                let rail = view.update(cx, |v, _| v.rail_list.viewport_bounds()).unwrap_or_default();
                let (t0, c0, f0) = (Instant::now(), cpu_ns(), counts(cx));
                for i in 0..125 {
                    let y = rail.top() + rail.size.height * (i as f32 / 125.);
                    let _ = cx.update(|window, cx| {
                        window.dispatch_event(
                            PlatformInput::MouseMove(MouseMoveEvent {
                                position: point(rail.left() + px(60.), y),
                                pressed_button: None,
                                modifiers: Modifiers::default(),
                            }),
                            cx,
                        );
                    });
                    sleep(0.016).await;
                }
                sleep(0.3).await;
                out["hover_sweep"] = stats(cx, t0, Instant::now(), c0, f0);
                out["hover_sweep"]["rail_items"] = json!(view.update(cx, |v, _| v.rail.rail.items.len()).unwrap_or(0));
                let theme = |cx: &mut AsyncWindowContext, name: &'static str| {
                    let _ = cx.update(|_, cx| {
                        if let Some((_, raw)) = crate::theme::builtin(name) {
                            crate::theme::update(cx, |c| {
                                c.theme = name.into();
                                c.raw = raw;
                            });
                        }
                    });
                };
                let (t0, c0, f0) = (Instant::now(), cpu_ns(), counts(cx));
                theme(cx, "night-city");
                sleep(1.0).await;
                out["cross_fade"] = stats(cx, t0, Instant::now(), c0, f0);
                theme(cx, "nord");
                sleep(1.0).await;
                // polish.md §6 gate 4: 5 sessions × 5 workers, 30 s. Five
                // busy sessions in the rail, each with five workers; the
                // shown one's five in its tray and transcript, running.
                let _ = upd(cx, &|v, cx| {
                    let metas: Vec<SessionMeta> = (0..5)
                        .filter_map(|k| {
                            serde_json::from_value(
                                json!({"id": if k == 0 { "bench".to_string() } else { format!("20261117-0900{k:02}-l{k:03}") },
                                "title": format!("Load {}", k + 1), "model": "m", "lifecycle": "live", "busy": true, "subagents_running": 5,
                                "messages": 12, "created_at": format!("2026-11-17T09:00:{k:02}Z")}),
                            )
                            .ok()
                        })
                        .collect();
                    v.apply_batch(vec![Event::Sessions(metas)], cx);
                    let mut evs = Vec::new();
                    let mut rows = Vec::new();
                    for n in 2..7u64 {
                        let (id, name) = (format!("w{n}"), format!("worker-{n}"));
                        let res = json!({"handle_id": format!("sa_{n}"), "agent_name": name, "status": "running"}).to_string();
                        evs.push(json!({"ev": "stream", "event": {"kind": "llm", "llm": "tool_use", "tool_name": "subagent_start",
                            "tool_id": id, "input": {"agent": name, "task": "go"}}}));
                        evs.push(json!({"ev": "stream", "event": {"kind": "llm", "llm": "tool_result", "tool_id": id, "result": res}}));
                        rows.push(json!({"subagent_id": n, "agent_name": name, "status": "running", "cancel_requested": false,
                            "elapsed_secs": 3.0, "finished_elapsed": null, "step": "reading src", "tools": 2}));
                    }
                    evs.push(json!({"ev": "subagent_rows", "rows": rows}));
                    v.apply_batch(evs.into_iter().filter_map(local_raw).collect(), cx);
                    v.sync_list();
                    v.list.scroll_to_end();
                });
                sleep(2.0).await;
                let (t0, c0, f0) = (Instant::now(), cpu_ns(), counts(cx));
                sleep(30.0).await;
                out["load_5x5_30s"] = stats(cx, t0, Instant::now(), c0, f0);
                out["load_5x5_30s"]["workers_running"] =
                    json!(view.update(cx, |v, _| v.model.stats.subagents.running_count()).unwrap_or(0));
                out["load_5x5_30s"]["sites"] = json!(view.update(cx, |v, _| v.fx.site_count()).unwrap_or(0));
                out["mem_end"] = mem::sample();
                out["total_s"] = json!(round(t_main.elapsed().as_secs_f64(), 2));
                println!("{out}");
                let _ = cx.update(|_, cx| cx.quit());
                return;
            }
            if opts.diag {
                let stats = |cx: &mut AsyncWindowContext, t0: Instant, t1: Instant| {
                    view.update(cx, |v, _| {
                        let b = v.bench.as_mut().unwrap();
                        b.drain();
                        b.stats(t0, t1, refresh_fallback)
                    })
                    .unwrap_or(Value::Null)
                };
                let _ = upd(cx, &|v, _| v.bench.as_mut().unwrap().animate = true);
                sleep(0.2).await;
                let t0 = Instant::now();
                sleep(1.0).await;
                out["static_empty"] = stats(cx, t0, Instant::now());
                let _ = upd(cx, &|v, _| {
                    v.model.set_local_session("diag");
                    v.model.transcript.apply_history(&dated(synthetic::history(opts.history_messages, 11)));
                    v.sync_list();
                    v.list.set_follow_mode(FollowMode::Normal);
                    v.list.scroll_to(ListOffset { item_ix: 0, offset_in_item: px(0.) });
                });
                sleep(0.4).await;
                let t0 = Instant::now();
                sleep(1.0).await;
                out["static_long_md"] = stats(cx, t0, Instant::now());
                for (name, plain) in [("scroll_plain", true), ("scroll_markdown", false)] {
                    let _ = upd(cx, &move |v, _| {
                        v.plain_rows = plain;
                        v.bench.as_mut().unwrap().scroll = Some((Instant::now(), opts.scroll_px_per_s));
                    });
                    let t0 = Instant::now();
                    sleep(2.5).await;
                    out[name] = stats(cx, t0, Instant::now());
                    let _ = upd(cx, &|v, _| v.bench.as_mut().unwrap().scroll = None);
                }
                let _ = upd(cx, &|v, _| {
                    v.plain_rows = false;
                    v.list.scroll_to(ListOffset { item_ix: 0, offset_in_item: px(0.) });
                    v.bench.as_mut().unwrap().scroll = Some((Instant::now(), opts.scroll_px_per_s));
                });
                let t0 = Instant::now();
                sleep(2.5).await;
                out["scroll_markdown_from_top_again"] = stats(cx, t0, Instant::now());
                let _ = upd(cx, &|v, _| v.bench.as_mut().unwrap().scroll = None);
                for (name, plain, seed) in [("stream_plain", true, 21u64), ("stream_markdown", false, 22)] {
                    let _ = upd(cx, &move |v, _| {
                        v.plain_rows = plain;
                        v.model.set_local_session("diag-stream");
                        v.reset_transcript();
                        v.model.transcript.apply_history(&dated(synthetic::history(40, 3)));
                        v.model.transcript.apply(&SessionEvent::Stream { event: StreamEvent::Llm(LlmEvent::ResponseStart) });
                        v.sync_list();
                        v.list.set_follow_mode(FollowMode::Tail);
                        v.list.scroll_to_end();
                    });
                    let n = 500usize;
                    let deltas = synthetic::deltas(n * 9, seed);
                    let ftx = tx.clone();
                    let feeder = std::thread::spawn(move || {
                        let start = Instant::now();
                        for (i, d) in deltas.into_iter().take(n).enumerate() {
                            let due = start + Duration::from_micros(5_000 * i as u64);
                            if let Some(w) = due.checked_duration_since(Instant::now()) {
                                std::thread::sleep(w);
                            }
                            let ev = SessionEvent::Stream { event: StreamEvent::Llm(LlmEvent::Text { text: d }) };
                            let _ = ftx.try_send(local_event(i as u64, ev));
                        }
                    });
                    let t0 = Instant::now();
                    sleep(2.5).await;
                    let _ = feeder.join();
                    out[name] = stats(cx, t0, Instant::now());
                }
                out["mem_end"] = mem::sample();
                out["startup_marks_ms"] = crate::diag::marks();
                out["total_s"] = json!(round(t_main.elapsed().as_secs_f64(), 2));
                println!("{out}");
                let _ = cx.update(|_, cx| cx.quit());
                return;
            }

            // 2. Streaming markdown at ~200 deltas/s into a short transcript.
            let _ = upd(cx, &|v, _| {
                v.model.set_local_session("bench-stream");
                v.model.transcript.apply_history(&dated(synthetic::history(40, 3)));
                v.model.transcript.apply(&SessionEvent::TurnStarted {
                    turn_baseline: 40,
                    trigger: "user".into(),
                    user_text: Some("Explain the frame pipeline, with code.".into()),
                });
                v.model.transcript.apply(&SessionEvent::Stream { event: StreamEvent::Llm(LlmEvent::ResponseStart) });
                v.sync_list();
                v.list.set_follow_mode(FollowMode::Tail);
                v.list.scroll_to_end();
                v.bench.as_mut().unwrap().animate = true;
            });
            sleep(0.3).await;
            let n = (opts.stream_secs * 200.0) as usize;
            let deltas = synthetic::deltas(n * 9, 5);
            let feeder_tx = tx.clone();
            let t0 = Instant::now();
            let feeder = std::thread::spawn(move || {
                let start = Instant::now();
                for (i, d) in deltas.into_iter().take(n).enumerate() {
                    let due = start + Duration::from_micros(5_000 * i as u64);
                    if let Some(w) = due.checked_duration_since(Instant::now()) {
                        std::thread::sleep(w);
                    }
                    let ev = SessionEvent::Stream { event: StreamEvent::Llm(LlmEvent::Text { text: d }) };
                    let _ = feeder_tx.try_send(local_event(i as u64, ev));
                }
                start.elapsed()
            });
            sleep(opts.stream_secs + 0.1).await;
            let t1 = Instant::now();
            let fed = feeder.join().unwrap_or_default();
            for ev in [SessionEvent::Stream { event: StreamEvent::Session(SessionStreamEvent::Done) }, SessionEvent::Idle] {
                let _ = tx.try_send(local_event(0, ev));
            }
            sleep(0.4).await;
            let stream = view
                .update(cx, |v, _| {
                    let b = v.bench.as_mut().unwrap();
                    b.drain();
                    b.animate = false;
                    let mut s = b.stats(t0, t1, refresh_fallback);
                    s["deltas_sent"] = json!(n);
                    s["deltas_per_s"] = json!(round(n as f64 / fed.as_secs_f64().max(1e-3), 1));
                    s["deltas_received"] = json!(b.received_deltas);
                    s["streamed_chars"] = json!(v.model.transcript.items().last().map(|i| i.text.len()));
                    s
                })
                .unwrap_or(Value::Null);
            out["stream"] = stream;
            out["mem_after_stream"] = mem::sample();

            // 3. A 2,000-message transcript with code blocks, scrolled top → down.
            let _ = upd(cx, &|v, _| {
                v.model.set_local_session("bench-long");
                v.reset_transcript();
                v.model.transcript.apply_history(&dated(synthetic::history(opts.history_messages, 11)));
                v.sync_list();
                v.list.set_follow_mode(FollowMode::Normal);
                v.list.scroll_to(ListOffset { item_ix: 0, offset_in_item: px(0.) });
                v.bench.as_mut().unwrap().animate = true;
            });
            sleep(0.6).await;
            out["mem_long_transcript_loaded"] = mem::sample();
            let speed = opts.scroll_px_per_s;
            let t0 = Instant::now();
            let _ = upd(cx, &|v, _| v.bench.as_mut().unwrap().scroll = Some((Instant::now(), speed)));
            sleep(opts.scroll_secs).await;
            let t1 = Instant::now();
            let scroll = view
                .update(cx, |v, _| {
                    let b = v.bench.as_mut().unwrap();
                    b.scroll = None;
                    b.animate = false;
                    b.drain();
                    let mut s = b.stats(t0, t1, refresh_fallback);
                    s["rows"] = json!(v.model.transcript.len());
                    s["messages"] = json!(opts.history_messages);
                    s["px_per_s"] = json!(speed);
                    s["reached_row"] = json!(v.list.logical_scroll_top().item_ix);
                    s
                })
                .unwrap_or(Value::Null);
            out["scroll"] = scroll;
            sleep(0.3).await;
            out["mem_after_scroll"] = mem::sample();

            // 3b. A wheel spun over the same transcript (fluid.md §3.2): a
            //     notch (3 lines) every 25 ms for 2.5 s, through the
            //     window's own input path, first under Full (the scroller's
            //     glide), then Reduced (GPUI's own wheel: the build before F1).
            let center = view.update(cx, |v, _| v.list.viewport_bounds().center()).unwrap_or_default();
            for (name, level) in [("spin_full", crate::motion::Level::Full), ("spin_reduced", crate::motion::Level::Reduced)] {
                let _ = cx.update(|_, cx| {
                    cx.set_global(crate::motion::Setting { level: Some(level), lock: None });
                    crate::motion::Level::apply(cx);
                });
                let _ = upd(cx, &|v, _| v.bench.as_mut().unwrap().animate = false);
                sleep(0.3).await;
                let t0 = Instant::now();
                let start = view.update(cx, |v, _| -f32::from(v.list.scroll_px_offset_for_scrollbar().y)).unwrap_or(0.);
                for _ in 0..100 {
                    let _ = cx.update(|window, cx| {
                        window.dispatch_event(
                            PlatformInput::MouseMove(MouseMoveEvent {
                                position: center,
                                pressed_button: None,
                                modifiers: Modifiers::default(),
                            }),
                            cx,
                        );
                        window.dispatch_event(
                            PlatformInput::Scroll(ScrollEvent {
                                position: center,
                                delta: ScrollDelta::Lines(point(0., -3.)),
                                modifiers: Modifiers::default(),
                                touch_phase: TouchPhase::Moved,
                                source: ScrollSource::Pointer,
                            }),
                            cx,
                        );
                    });
                    sleep(0.025).await;
                }
                let t1 = Instant::now();
                sleep(0.3).await;
                out[name] = view
                    .update(cx, |v, _| {
                        let b = v.bench.as_mut().unwrap();
                        b.drain();
                        let mut s = b.stats(t0, t1, refresh_fallback);
                        s["travel_px"] = json!(round((-f32::from(v.list.scroll_px_offset_for_scrollbar().y) - start) as f64, 1));
                        s
                    })
                    .unwrap_or(Value::Null);
            }
            let _ = cx.update(|_, cx| {
                cx.set_global(crate::motion::Setting { level: Some(crate::motion::Level::Full), lock: None });
                crate::motion::Level::apply(cx);
            });

            // 4. A 400-turn transcript at its end, both side panels' edges
            //    dragged across their limits (every frame a new centre
            //    width: the visible rows re-wrap).
            let _ = upd(cx, &|v, _| {
                v.model.set_local_session("bench-drag");
                v.reset_transcript();
                v.model.transcript.apply_history(&turns(DRAG_TURNS, 13));
                v.sync_list();
                v.list.set_follow_mode(FollowMode::Tail);
                v.list.scroll_to_end();
            });
            sleep(0.4).await;
            for (name, rail) in [("drag_rail", true), ("drag_right", false)] {
                let t0 = Instant::now();
                let _ = upd(cx, &move |v, _| {
                    let b = v.bench.as_mut().unwrap();
                    b.drag = Some((t0, rail));
                    b.animate = true;
                });
                sleep(opts.drag_secs).await;
                let t1 = Instant::now();
                out[name] = view
                    .update(cx, |v, _| {
                        let b = v.bench.as_mut().unwrap();
                        b.drag = None;
                        b.animate = false;
                        b.drain();
                        let mut s = b.stats(t0, t1, refresh_fallback);
                        s["rows"] = json!(v.model.transcript.len());
                        s["turns"] = json!(DRAG_TURNS);
                        s["px_per_s"] = json!(DRAG_PX_PER_S);
                        s["window_w"] = json!(v.width);
                        s
                    })
                    .unwrap_or(Value::Null);
                sleep(0.2).await;
            }

            // 5. One real turn against the daemon, if connected.
            let has_client = view.update(cx, |v, _| v.client.is_some()).unwrap_or(false);
            if let (true, Some(prompt)) = (has_client, opts.prompt.clone()) {
                let t_create = Instant::now();
                let _ = upd(cx, &|v, _| {
                    let seq = v.client.as_ref().unwrap().create(synaps_client::CreateOptions::new());
                    if let Ok(seq) = seq {
                        v.model.want(seq);
                    }
                });
                let mut attached = None;
                for _ in 0..250 {
                    sleep(0.02).await;
                    attached = view.update(cx, |v, _| v.bench.as_ref().unwrap().attached_at).ok().flatten();
                    if attached.is_some() {
                        break;
                    }
                }
                let mut real = json!({"attach_ms": attached.map(|a| round(ms(a.duration_since(t_create)), 1))});
                if attached.is_some() {
                    sleep(0.2).await;
                    let submit = Instant::now();
                    let p = prompt.clone();
                    let _ = upd(cx, &move |v, _| {
                        v.bench.as_mut().unwrap().submit_at = Some(submit);
                        let _ = v.client.as_ref().unwrap().submit(p.clone());
                    });
                    let budget = 28.0 - t_main.elapsed().as_secs_f64();
                    let until = Instant::now() + Duration::from_secs_f64(budget.max(1.0));
                    while Instant::now() < until {
                        sleep(0.02).await;
                        let done = view.update(cx, |v, _| v.bench.as_ref().unwrap().idle_at.is_some()).unwrap_or(true);
                        if done {
                            break;
                        }
                    }
                    sleep(0.1).await;
                    real = view
                        .update(cx, |v, _| {
                            let b = v.bench.as_mut().unwrap();
                            b.drain();
                            let painted = b.first_text_at.and_then(|t| b.first_present_after(t));
                            json!({
                                "attach_ms": attached.map(|a| round(ms(a.duration_since(t_create)), 1)),
                                "ttft_ms": b.first_text_at.map(|t| round(ms(t.duration_since(submit)), 1)),
                                "ttft_painted_ms": painted.map(|t| round(ms(t.duration_since(submit)), 1)),
                                "turn_ms": b.idle_at.map(|t| round(ms(t.duration_since(submit)), 1)),
                                "rows": v.model.transcript.len(),
                            })
                        })
                        .unwrap_or(Value::Null);
                }
                out["daemon_turn"] = real;
            }
            out["mem_end"] = mem::sample();
            out["startup_marks_ms"] = crate::diag::marks();
            out["total_s"] = json!(round(t_main.elapsed().as_secs_f64(), 2));
            println!("{out}");
            let _ = cx.update(|_, cx| cx.quit());
        })
        .detach();
}

/// A synthetic event on the bench's local session (see
/// `Model::set_local_session`), sent now: its rows carry the time, as a
/// daemon's do.
/// A raw daemon event of the bench's local session, decoded as the client
/// decodes one.
fn local_raw(raw: Value) -> Option<Event> {
    let event = synaps_client::protocol::decode_event(&raw).ok()?;
    Some(Event::session(LOCAL_SEQ, "bench", 1, event, Arc::new(raw)))
}

fn local_event(seq: u64, event: SessionEvent) -> Event {
    let ts = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Nanos, true);
    Event::session(LOCAL_SEQ, "bench", seq, event, Arc::new(Value::Null)).with_ts(ts)
}

/// `h` with message times (`_synaps_at`) a minute apart from a fixed
/// start, as a daemon since 0.11 records them: the bench's rows carry
/// times and day lines as a real session's do.
fn dated(mut h: Vec<Value>) -> Vec<Value> {
    let t0 = chrono::DateTime::from_timestamp(1_794_556_800, 0).unwrap_or_default();
    for (i, m) in h.iter_mut().enumerate() {
        let at = t0 + chrono::Duration::minutes(i as i64);
        m[synaps_client::transcript::MESSAGE_AT_KEY] = json!(at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
    }
    h
}
