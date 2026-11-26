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
    /// cached and not, then RSS with 1 to [`LIVES`] stores attached at once).
    pub tabs: bool,
    /// `--bench-history FILE`: the tab bench's sessions all have this
    /// history (a saved session's `api_messages`: a real one, with its
    /// tool results and edits), not the synthetic one.
    pub history_file: Option<std::path::PathBuf>,
    /// `--bench-motion`: what motion costs (motion.md §9): presents over
    /// 10 s idle, then a running tool's heartbeat (frames, phases, CPU per
    /// frame), then the same with Motion off.
    pub motion: bool,
    /// `--bench-panels`: the side panels folded and unfolded and the
    /// composer grown a line and back, over a long transcript (polish.md
    /// P14): each one's frames, draw times and CPU, then what follows it.
    pub panels: bool,
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
    /// The real-daemon bench's own attachment (its prompt goes there).
    pub attach_seq: Option<u64>,
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
            attach_seq: None,
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
/// `--bench-tabs`' `mem_lives`: stores attached at once, up to this many.
const LIVES: usize = 3;
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
    let limit = if opts.motion { 400 } else { 30 };
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
                let rows = view.update(cx, |v, _| v.model.lives[v.panes.cur.live].transcript.len()).unwrap_or(0);
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
                    let rows_at_switch = view.update(cx, |v, _| v.model.lives[v.panes.cur.live].transcript.len()).unwrap_or(0);
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
                // Split panes step 2: what one more attachment held at once
                // costs. The tab's store stays as it is; stores 2 and 3 (step
                // 3's other panes') each attach a session of their own, of
                // the same turns, as the synthetic daemon would answer it.
                let mut mem_lives = json!({});
                let sample = |cx: &mut AsyncWindowContext| {
                    let mut m = mem::sample();
                    m["trimmed_rss_kb"] = mem::trimmed_rss_kb();
                    let rows: Vec<usize> = view.update(cx, |v, _| v.model.lives.iter().map(|l| l.transcript.len()).collect()).unwrap_or_default();
                    m["stores_rows"] = json!(rows);
                    m
                };
                sleep(0.5).await;
                mem_lives["lives_1"] = sample(cx);
                for n in 2..=LIVES {
                    let sid = format!("20261114-0901{n:02}-l{n:03}");
                    let Some(ev) = ({
                        let sid = sid.clone();
                        view.update(cx, move |v, _| {
                            let live = v.model.lives.add();
                            v.synthetic_seq += 1;
                            let seq = v.synthetic_seq;
                            v.model.attaching(live, seq, None, false);
                            v.model.asked_for(live, &sid);
                            attached_event(&sid, seq, &turns(DRAG_TURNS, 60 + n as u64))
                        })
                        .ok()
                        .flatten()
                    }) else {
                        break;
                    };
                    let _ = tx.try_send(ev);
                    let _ = wait_attached(cx, &sid).await;
                    sleep(0.5).await;
                    mem_lives[format!("lives_{n}")] = sample(cx);
                }
                out["mem_lives"] = mem_lives;
                out["mem_end"] = mem::sample();
                out["total_s"] = json!(round(t_main.elapsed().as_secs_f64(), 2));
                println!("{out}");
                let _ = cx.update(|_, cx| cx.quit());
                return;
            }
            if opts.panels {
                // A 400-turn transcript at its end; each action, then its
                // next 0.8 s (presents, draws, CPU, app frames) and the 1 s
                // after it (nothing moves: 0).
                let frames = |cx: &mut AsyncWindowContext| view.update(cx, |v, _| v.fx.frames()).unwrap_or(0);
                let _ = upd(cx, &|v, _| {
                    v.model.local_session(v.panes.cur.live, "bench-panels");
                    v.reset_transcript();
                    v.model.lives[v.panes.cur.live].transcript.apply_history(&turns(DRAG_TURNS, 13));
                    v.sync_list();
                    v.panes.cur.list.set_follow_mode(FollowMode::Tail);
                    v.panes.cur.list.scroll_to_end();
                });
                sleep(1.5).await;
                // `SYNAPS_BENCH_PANES_ACROSS=N` (the 2026-11-25 note under
                // split.md §0): N − 1 panes side by side first, then a split
                // to N before the panels' actions and a close back after.
                let across = std::env::var("SYNAPS_BENCH_PANES_ACROSS").ok().and_then(|s| s.parse::<usize>().ok()).filter(|n| *n > 1);
                for _ in 2..across.unwrap_or(0) {
                    let _ = view.update_in(cx, |v, w, cx| {
                        v.split_pane(Some(crate::split::Side::Right), w, cx);
                        v.keys_to = None;
                        cx.notify();
                    });
                    sleep(0.6).await;
                }
                if across.is_some() {
                    sleep(1.0).await;
                }
                let long = "A draft that runs past the end of its first line and onto a second one, so the composer grows by \
                            a line: the field's height and the transcript above it make room, then it shrinks back once cleared.";
                let panels = [
                    ("rail_fold", 0u8),
                    ("rail_unfold", 0),
                    ("right_fold", 1),
                    ("right_unfold", 1),
                    ("composer_grow", 2),
                    ("composer_shrink", 3),
                ];
                let acts: Vec<(&str, u8)> = match across {
                    Some(_) => [("split", 4u8)].into_iter().chain(panels).chain([("close", 5)]).collect(),
                    None => panels.to_vec(),
                };
                for (name, act) in acts {
                    let (t0, c0, f0) = (Instant::now(), cpu_ns(), frames(cx));
                    let _ = cx.update(|window, cx| {
                        let _ = view.update(cx, |v, cx| {
                            match act {
                                0 => v.toggle_panel(true, cx),
                                1 => v.toggle_panel(false, cx),
                                2 => v.panes.cur.composer.update(cx, |s, cx| s.set_value(long, window, cx)),
                                3 => v.panes.cur.composer.update(cx, |s, cx| s.set_value("", window, cx)),
                                4 => {
                                    v.split_pane(Some(crate::split::Side::Right), window, cx);
                                    v.keys_to = None;
                                }
                                _ => v.close_pane(),
                            }
                            cx.notify();
                        });
                    });
                    sleep(0.8).await;
                    let (t1, cpu, f1) = (Instant::now(), cpu_ns().saturating_sub(c0) as f64 / 1e6, frames(cx));
                    sleep(1.0).await;
                    let (t2, f2) = (Instant::now(), frames(cx));
                    let mut s = view
                        .update(cx, |v, _| {
                            let b = v.bench.as_mut().unwrap();
                            b.drain();
                            let mut s = b.stats(t0, t1, refresh_fallback);
                            s["presents_after"] = b.stats(t1, t2, refresh_fallback)["frames_presented"].clone();
                            s
                        })
                        .unwrap_or(Value::Null);
                    s["cpu_ms"] = json!(round(cpu, 2));
                    s["app_frames"] = json!(f1 - f0);
                    s["app_frames_after"] = json!(f2 - f1);
                    out[name] = s;
                }
                out["rows"] = json!(view.update(cx, |v, _| v.model.lives[v.panes.cur.live].transcript.len()).unwrap_or(0));
                out["panes_across"] = json!(across.unwrap_or(1));
                out["window_w"] = json!(view.update(cx, |v, _| v.width).unwrap_or(0.));
                out["mem_end"] = mem::sample();
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
                // `SYNAPS_BENCH_LIFE=N` (Q3 evidence, polish.md §3 Life): after
                // N s, a scripted turn on the bench's session, the scene a
                // capture records: a bash call runs (the spinner, its clock,
                // its orbit, the composer's), workers spawn, run (their tool counts
                // and steps roll) and end (the check writes on), a call fails
                // (the cross), the fan-out widens past the tray's lines (`+N
                // more` rolls), a prompt comes (NEEDS YOU's count rolls), a
                // rail row is favourited (the star hops once it has landed),
                // the turn ends. Every event stamped, as a daemon's are.
                // `SYNAPS_BENCH_LIFE_LEVEL=full|reduced|off`: the level.
                if let Some(after) = std::env::var("SYNAPS_BENCH_LIFE").ok().and_then(|s| s.parse::<f64>().ok()) {
                    let lv = std::env::var("SYNAPS_BENCH_LIFE_LEVEL").ok().and_then(|s| crate::motion::Level::parse(&s));
                    level(cx, lv.unwrap_or(crate::motion::Level::Full));
                    let apply = |cx: &mut AsyncWindowContext, evs: Vec<Value>| {
                        let ts = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Nanos, true);
                        let _ = upd(cx, &|v, cx| {
                            v.apply_batch(evs.iter().cloned().filter_map(local_raw).map(|e| e.with_ts(ts.clone())).collect(), cx);
                            v.sync_list();
                        });
                    };
                    let row = |n: u64, name: &str, status: &str, tools: u64, step: &str, secs: f64| {
                        let done = (status != "running").then_some(secs);
                        json!({"subagent_id": n, "agent_name": name, "status": status, "cancel_requested": false,
                            "elapsed_secs": secs, "finished_elapsed": done, "step": step, "tools": tools})
                    };
                    let spawn = |n: u64, name: &str, task: &str| {
                        let id = format!("w{n}");
                        let res = json!({"handle_id": format!("sa_{n}"), "agent_name": name, "status": "running"}).to_string();
                        vec![
                            json!({"ev": "stream", "event": {"kind": "llm", "llm": "tool_use", "tool_name": "subagent_start",
                                "tool_id": id, "input": {"agent": name, "task": task}}}),
                            json!({"ev": "stream", "event": {"kind": "llm", "llm": "tool_result", "tool_id": id, "result": res}}),
                        ]
                    };
                    let call = |id: &str, tool: &str, input: Value| {
                        json!({"ev": "stream", "event": {"kind": "llm", "llm": "tool_use", "tool_name": tool, "tool_id": id, "input": input}})
                    };
                    let result = |id: &str, text: &str| json!({"ev": "stream", "event": {"kind": "llm", "llm": "tool_result", "tool_id": id, "result": text}});
                    let _ = upd(cx, &|v, cx| {
                        v.model.local_session(v.panes.cur.live, "bench");
                        // Its session is this window's to drive: its turn's end
                        // hands the floor back (Handoff in, the settle pass, P3).
                        v.owner_here = true;
                        v.model.lives[v.panes.cur.live].transcript.apply_history(&dated(synthetic::history(12, 3)));
                        let metas: Vec<SessionMeta> = [
                            ("bench", "Relay refresh race", true, 0),
                            ("20261117-090001-l001", "Flaky e2e", true, 0),
                            ("20261117-090002-l002", "Docs sweep", false, 2),
                            ("20261117-090003-l003", "Idle CPU", false, 0),
                        ]
                        .iter()
                        .enumerate()
                        .filter_map(|(k, (id, title, busy, subs))| {
                            serde_json::from_value(json!({"id": id, "title": title, "model": "m", "lifecycle": "live", "busy": busy,
                                "subagents_running": subs, "messages": 12, "created_at": format!("2026-11-17T09:00:{k:02}Z")}))
                            .ok()
                        })
                        .collect();
                        v.apply_batch(vec![Event::Sessions(metas)], cx);
                        v.sync_list();
                        v.panes.cur.list.set_follow_mode(FollowMode::Tail);
                        v.panes.cur.list.scroll_to_end();
                    });
                    sleep(after).await;
                    let _ = upd(cx, &|v, _| {
                        let t = &mut v.model.lives[v.panes.cur.live].transcript;
                        t.apply(&SessionEvent::TurnStarted {
                            turn_baseline: 12,
                            trigger: "user".into(),
                            user_text: Some("Find the refresh race and fix it.".into()),
                        });
                        t.apply(&SessionEvent::Stream { event: StreamEvent::Llm(LlmEvent::ResponseStart) });
                        v.sync_list();
                        v.panes.cur.list.scroll_to_end();
                    });
                    // `SYNAPS_BENCH_STATES=1` (H2 evidence, indicators/proposal.md
                    // §6): instead, one run with a call in each of the
                    // indicator's states (waiting, queued, running, streaming,
                    // done, failed, at its limit, denied, cancelled), held 5 s,
                    // then the live ones change (the quarter-turn), and the turn ends.
                    if std::env::var("SYNAPS_BENCH_STATES").is_ok() {
                        let _ = upd(cx, &|v, _| v.slow_daemon = true);
                        let delta = |id: &str, text: &str| {
                            json!({"ev": "stream", "event": {"kind": "llm", "llm": "tool_result_delta", "tool_id": id, "delta": text}})
                        };
                        let queued = |id: &str| json!({"ev": "stream", "event": {"kind": "llm", "llm": "tool_queued", "tool_id": id}});
                        let started = |id: &str| json!({"ev": "stream", "event": {"kind": "llm", "llm": "tool_started", "tool_id": id}});
                        let cmd = |c: &str| json!({"command": c});
                        apply(
                            cx,
                            vec![
                                call("w0", "bash", cmd("rm -rf target/tmp")),
                                call("q1", "bash", cmd("cargo build -p relay")),
                                queued("q1"),
                                call("r2", "bash", cmd("cargo test -p relay --test refresh_race")),
                                call("s3", "bash", cmd("cargo test -p session")),
                                delta("s3", "running 6 tests\n"),
                                call("d4", "read", json!({"path": "crates/relay/src/session.rs"})),
                                result("d4", "1  use std::sync::Arc;\n2  use tokio::sync::Mutex;"),
                                call("f5", "bash", cmd("cargo clippy -p relay")),
                                result("f5", "Tool execution failed: Command failed (exit 101):\nerror: unused variable `token`"),
                                call("l6", "bash", cmd("cat logs/relay.log")),
                                result("l6", "relay up\nrelay up\n[output truncated at 30000 bytes]"),
                                call("x7", "edit", json!({"path": "crates/relay/src/upstream.rs"})),
                                result("x7", "Tool call denied: guard"),
                                call("c8", "bash", cmd("cargo bench -p relay")),
                                result("c8", "Canceled by user"),
                                call("k9", "bash", cmd("cargo run -p relay -- --watch")),
                            ],
                        );
                        let _ = upd(cx, &|v, _| {
                            if let Ok(request) = serde_json::from_value(json!({
                                "id": 7, "kind": "confirm", "title": "Run a command?", "prompt": "Tool: bash\nInput that will run:\n$ rm -rf target/tmp"
                            })) {
                                v.model.lives[v.panes.cur.live].transcript.apply(&SessionEvent::Prompt { request });
                            }
                            v.sync_list();
                            v.panes.cur.list.scroll_to_end();
                        });
                        for k in 0..10 {
                            sleep(0.5).await;
                            apply(cx, vec![delta("s3", &format!("test refresh_{k} ... ok\n"))]);
                        }
                        // The live ones change: queued starts, running ends,
                        // one is cancelled, streaming fails, the prompt is answered.
                        apply(
                            cx,
                            vec![started("q1"), result("r2", "running 4 tests\ntest result: ok. 4 passed; 0 failed"), result("k9", "Canceled by user")],
                        );
                        let _ = upd(cx, &|v, cx| {
                            // Not `sync_list` here: the frame resolves the card
                            // first (it folds into its row), as a daemon's event.
                            v.model.lives[v.panes.cur.live].transcript.apply(&SessionEvent::PromptResolved { prompt_id: 7 });
                            cx.notify();
                        });
                        // `SYNAPS_BENCH_STATES=denied|halted` (#25): what the
                        // daemon applied was not this window's answer: another
                        // client's Deny came first (the gated call is blocked
                        // at its gate), or a HALT crossed it (cancelled before
                        // it ran). Answer Allow before the resolve to see it.
                        let crossed = match std::env::var("SYNAPS_BENCH_STATES").as_deref() {
                            Ok("denied") => Some("Tool call blocked by extension: Tool call confirmation denied: guard wants to check rm -rf"),
                            Ok("halted") => Some("Canceled by user"),
                            _ => None,
                        };
                        if let Some(text) = crossed {
                            sleep(0.1).await;
                            apply(cx, vec![result("w0", text)]);
                        }
                        // `SYNAPS_BENCH_HELD=1` (Shady #23 r4 H1): the room the
                        // card left held at the list's end, the composer
                        // grows three lines (a steer), then a new call at
                        // the run's end streams until it has used the room
                        // up, and on (the list follows; #23 r6's h5 film).
                        // `SYNAPS_BENCH_HELD=still`: the composer stays.
                        // `SYNAPS_BENCH_HELD=own` (Shady #23 r6 H1 (a)): the
                        // composer stays, and the run's own running call
                        // (`s3`) streams on instead, each line splicing its
                        // row (the list's top row in a short window).
                        // Marks on stderr: `HELD <what> <s>`.
                        if std::env::var("SYNAPS_BENCH_HELD").is_ok() {
                            let t0 = Instant::now();
                            let mark = |what: &str| eprintln!("HELD {what} {:.3}", t0.elapsed().as_secs_f64());
                            sleep(1.5).await;
                            mark("composer");
                            let how = std::env::var("SYNAPS_BENCH_HELD").unwrap_or_default();
                            let grow = how != "still" && how != "own";
                            let _ = view.update_in(cx, |v, window, cx| {
                                if !grow {
                                    return;
                                }
                                v.panes.cur.composer.update(cx, |s, cx| s.set_value("steer: and the relay too\nkeep the old token\nthen run it again", window, cx))
                            });
                            sleep(1.5).await;
                            mark("stream");
                            let _ = upd(cx, &|v, _| {
                                let t = v.panes.cur.list.logical_scroll_top();
                                let id = v.model.lives[v.panes.cur.live].shown().items().iter().find(|i| i.tool.as_ref().is_some_and(|t| t.tool_id == "s3")).map(|i| i.id);
                                let s3 = v.panes.cur.disp.iter().position(|d| Some(d.id) == id && d.kind == crate::turns::Kind::Work);
                                eprintln!("HELD top {} +{:.0}, s3's row {s3:?}", t.item_ix, f32::from(t.offset_in_item));
                            });
                            // A new call at the run's end (or `s3`): its
                            // output grows into the room, then past it.
                            let into = if how == "own" { "s3" } else { "h0" };
                            if into == "h0" {
                                apply(cx, vec![call("h0", "bash", cmd("cargo test -p relay --test held")), started("h0")]);
                            }
                            for k in 0..48 {
                                sleep(0.25).await;
                                apply(cx, vec![delta(into, &format!("test held_{k} ... ok\n"))]);
                            }
                            mark("done");
                        }
                        sleep(1.5).await;
                        apply(cx, vec![result("s3", "Tool execution failed: Command failed (exit 1):\ntest refresh_10 ... FAILED")]);
                        sleep(1.5).await;
                        apply(cx, vec![result("q1", "Finished `dev` profile"), result("w0", "")]);
                        sleep(2.5).await;
                        // The turn ends: the run folds. Then 9.5 s for a
                        // capture's clicks (the run unfolds, a call opens).
                        let _ = upd(cx, &|v, _| {
                            v.model.lives[v.panes.cur.live].transcript.apply(&SessionEvent::Idle);
                            v.sync_list();
                        });
                        if std::env::var("SYNAPS_BENCH_FOLDS").is_ok() {
                            // `SYNAPS_BENCH_FOLDS=1` (the design gate of #23,
                            // N1, N3, S1; Shady #23 r3 H2): the folds a
                            // click makes, as its listener makes them
                            // (`toggle_run`, `toggle`), each mark on stderr
                            // (`FOLDS <what> <s>`, the bench's clock):
                            // - unfolded at the end; its header a third of
                            //   the way down the view, folded there;
                            // - unfolded there, no scroll (the cascade alone);
                            // - at the end, folded there (its header holds);
                            // - unfolded as the list is sent to its end (the
                            //   rows reveal under the view), folded there;
                            // - unfolded, at the end: the last call's body
                            //   opened (the list shows its first lines) and
                            //   closed;
                            // - a new turn's call streams into the held room
                            //   while the reader scrolls up 0.5 px a frame
                            //   (a touchpad) for 3 s, and after.
                            // `SYNAPS_BENCH_FOLDS=wrap`: the streamed lines
                            // are long, each wrapping (#26 L6).
                            let t0 = Instant::now();
                            let mark = |what: &str| eprintln!("FOLDS {what} {:.3}", t0.elapsed().as_secs_f64());
                            let run = |v: &AppView| v.panes.cur.disp.iter().rev().find(|d| matches!(d.kind, crate::turns::Kind::Summary { .. })).map(|d| d.id);
                            let last = |v: &AppView| v.panes.cur.disp.iter().rev().find(|d| d.kind == crate::turns::Kind::Work).map(|d| d.id);
                            let flip = |v: &mut AppView| {
                                if let Some(id) = run(v) {
                                    v.toggle_run(id);
                                }
                            };
                            sleep(2.5).await;
                            mark("unfold-end");
                            let _ = upd(cx, &|v, _| flip(v));
                            sleep(2.).await;
                            let _ = upd(cx, &|v, _| {
                                if let Some(ix) = run(v).and_then(|id| v.panes.cur.disp.iter().position(|d| d.id == id)) {
                                    let third = f32::from(v.panes.cur.list.viewport_bounds().size.height) / 3.;
                                    v.panes.cur.list.scroll_to(gpui::ListOffset { item_ix: ix, offset_in_item: gpui::px(0.) });
                                    v.panes.cur.list.scroll_by(gpui::px(-third));
                                }
                            });
                            sleep(1.5).await;
                            mark("fold-mid");
                            let _ = upd(cx, &|v, _| flip(v));
                            sleep(2.5).await;
                            mark("unfold-mid");
                            let _ = upd(cx, &|v, _| flip(v));
                            sleep(2.).await;
                            let _ = upd(cx, &|v, _| v.panes.cur.list.scroll_to_end());
                            sleep(1.).await;
                            mark("fold-end");
                            let _ = upd(cx, &|v, _| flip(v));
                            sleep(2.5).await;
                            mark("unfold-to-end");
                            let _ = upd(cx, &|v, _| {
                                flip(v);
                                v.panes.cur.list.scroll_to_end();
                            });
                            sleep(2.).await;
                            mark("fold-rows-under");
                            let _ = upd(cx, &|v, _| flip(v));
                            sleep(2.5).await;
                            mark("unfold-for-body");
                            let _ = upd(cx, &|v, _| flip(v));
                            sleep(1.5).await;
                            let _ = upd(cx, &|v, _| v.panes.cur.list.scroll_to_end());
                            sleep(1.).await;
                            mark("body-open");
                            let _ = upd(cx, &|v, _| {
                                if let Some(id) = last(v) {
                                    v.toggle(id);
                                }
                            });
                            sleep(2.).await;
                            mark("body-close");
                            let _ = upd(cx, &|v, _| {
                                if let Some(id) = last(v) {
                                    v.toggle(id);
                                }
                            });
                            sleep(2.).await;
                            mark("turn-stream");
                            let _ = upd(cx, &|v, _| {
                                let t = &mut v.model.lives[v.panes.cur.live].transcript;
                                t.apply(&SessionEvent::TurnStarted {
                                    turn_baseline: 14,
                                    trigger: "user".into(),
                                    user_text: Some("Tail the log.".into()),
                                });
                                t.apply(&SessionEvent::Stream { event: StreamEvent::Llm(LlmEvent::ResponseStart) });
                                v.sync_list();
                            });
                            apply(cx, vec![call("z9", "bash", cmd("tail -f logs/relay.log")), started("z9")]);
                            sleep(1.).await;
                            mark("slow-scroll");
                            let wrap = std::env::var("SYNAPS_BENCH_FOLDS").is_ok_and(|v| v == "wrap");
                            let line = |k: usize| {
                                if wrap {
                                    format!("relay: refresh {k} ok, {}\n", "token rotated, cache warmed, upstream answered 200 in 41 ms; ".repeat(3))
                                } else {
                                    format!("relay: refresh {k} ok\n")
                                }
                            };
                            for k in 0..430 {
                                let _ = upd(cx, &|v, _| {
                                    v.panes.cur.scroller.touch();
                                    v.panes.cur.list.scroll_by(gpui::px(-0.5));
                                });
                                if k % 40 == 0 {
                                    apply(cx, vec![delta("z9", &line(k))]);
                                }
                                sleep(0.007).await;
                            }
                            mark("scroll-stopped");
                            for k in 0..8 {
                                apply(cx, vec![delta("z9", &line(500 + k))]);
                                sleep(0.3).await;
                            }
                            mark("done");
                            println!("{out}");
                            let _ = cx.update(|_, cx| cx.quit());
                            return;
                        } else {
                            sleep(9.5).await;
                        }
                        // A new turn, detached with its call still running:
                        // as last seen.
                        let _ = upd(cx, &|v, _| {
                            let t = &mut v.model.lives[v.panes.cur.live].transcript;
                            t.apply(&SessionEvent::TurnStarted { turn_baseline: 14, trigger: "user".into(), user_text: Some("Tail the log.".into()) });
                            t.apply(&SessionEvent::Stream { event: StreamEvent::Llm(LlmEvent::ResponseStart) });
                            v.sync_list();
                            v.panes.cur.list.scroll_to_end();
                        });
                        apply(cx, vec![call("z9", "bash", cmd("tail -f logs/relay.log"))]);
                        sleep(1.5).await;
                        let _ = upd(cx, &|v, _| v.model.lives[v.panes.cur.live].active = None);
                        sleep(2.5).await;
                        println!("{out}");
                        let _ = cx.update(|_, cx| cx.quit());
                        return;
                    }
                    apply(cx, vec![call("b1", "bash", json!({"command": "cargo test -p relay --test refresh_race"}))]);
                    sleep(2.5).await;
                    // Each worker's clock is its time since it spawned, as a
                    // daemon's is (re-gate #17: never a clock going back).
                    let age = |at: Instant| round(at.elapsed().as_secs_f64(), 1);
                    let scout_at = Instant::now();
                    let mut evs = spawn(1, "scout", "Find every caller of TokenStore::rotate and say which hold the lock");
                    evs.push(json!({"ev": "subagent_rows", "rows": [row(1, "scout", "running", 1, "reading crates/relay/src/upstream.rs", 0.)]}));
                    apply(cx, evs);
                    sleep(2.0).await;
                    apply(cx, vec![json!({"ev": "subagent_rows", "rows": [row(1, "scout", "running", 2, "grep -n rotate crates/relay/src", age(scout_at))]})]);
                    sleep(2.0).await;
                    let names = ["reviewer", "fixer", "tester", "docs"];
                    let crew_at = Instant::now();
                    let mut evs = Vec::new();
                    for (k, name) in names.iter().enumerate() {
                        evs.extend(spawn(k as u64 + 2, name, "Review the retry path in Session for lost tokens"));
                    }
                    let mut rows = vec![row(1, "scout", "running", 3, "reading crates/relay/src/session.rs", age(scout_at))];
                    rows.extend(names.iter().enumerate().map(|(k, n)| row(k as u64 + 2, n, "running", 1, "reading src", 0.)));
                    evs.push(json!({"ev": "subagent_rows", "rows": rows}));
                    apply(cx, evs);
                    sleep(2.0).await;
                    apply(cx, vec![result("b1", "running 4 tests\ntest result: ok. 4 passed; 0 failed")]);
                    sleep(1.5).await;
                    apply(cx, vec![call("r1", "read", json!({"path": "crates/relay/src/gone.rs"}))]);
                    sleep(0.8).await;
                    apply(cx, vec![result("r1", "Tool execution failed: no such file")]);
                    sleep(1.5).await;
                    let scout_done = age(scout_at);
                    // Scout done; the four at their second tool, as of now.
                    let crew = || {
                        let mut rows = vec![row(1, "scout", "completed", 4, "", scout_done)];
                        rows.extend(names.iter().enumerate().map(|(k, n)| row(k as u64 + 2, n, "running", 2, "grep -n Stale src", age(crew_at))));
                        rows
                    };
                    apply(cx, vec![json!({"ev": "subagent_rows", "rows": crew()})]);
                    sleep(2.0).await;
                    // A wider fan-out (P5's `+N more`): seven more workers, more
                    // than the tray's chip lines hold, then one more (its count
                    // rolls).
                    let probe = |k: u64, secs: f64| row(k, &format!("probe-{k}"), "running", 1, "reading shard", secs);
                    let probes_at = Instant::now();
                    let (mut wide, mut evs) = (crew(), Vec::new());
                    for k in 6..13 {
                        evs.extend(spawn(k, &format!("probe-{k}"), "Probe one shard of the token store for stale entries"));
                        wide.push(probe(k, 0.));
                    }
                    evs.push(json!({"ev": "subagent_rows", "rows": wide}));
                    apply(cx, evs);
                    sleep(1.5).await;
                    // The eighth: its own spawn only (Shady N5).
                    let mut wide = crew();
                    wide.extend((6..13).map(|k| probe(k, age(probes_at))));
                    wide.push(probe(13, 0.));
                    let last_at = Instant::now();
                    let mut evs = spawn(13, "probe-13", "Probe one shard of the token store for stale entries");
                    evs.push(json!({"ev": "subagent_rows", "rows": wide}));
                    apply(cx, evs);
                    sleep(1.5).await;
                    let _ = upd(cx, &|v, _| {
                        if let Ok(request) = serde_json::from_value(json!({
                            "id": 7, "kind": "confirm", "title": "Run a command?", "prompt": "Tool: bash\nInput that will run:\n$ cargo test"
                        })) {
                            v.model.lives[v.panes.cur.live].transcript.apply(&SessionEvent::Prompt { request });
                        }
                        v.sync_list();
                    });
                    sleep(2.0).await;
                    let _ = upd(cx, &|v, _| v.toggle_fav_session("20261117-090001-l001"));
                    sleep(1.5).await;
                    // The prompt answered: RUNNING again (the composer's orbit).
                    let _ = upd(cx, &|v, cx| {
                        // Not `sync_list` here: the frame resolves the card
                        // first (it folds into its row), as a daemon's event.
                        v.model.lives[v.panes.cur.live].transcript.apply(&SessionEvent::PromptResolved { prompt_id: 7 });
                        cx.notify();
                    });
                    sleep(1.5).await;
                    // Every worker done (their checks write on).
                    let mut done = vec![row(1, "scout", "completed", 4, "", scout_done)];
                    let crew_done = age(crew_at);
                    done.extend(names.iter().enumerate().map(|(k, n)| row(k as u64 + 2, n, "completed", 3, "", crew_done)));
                    let probes_done = age(probes_at);
                    done.extend((6..13).map(|k| row(k, &format!("probe-{k}"), "completed", 2, "", probes_done)));
                    done.push(row(13, "probe-13", "completed", 2, "", age(last_at)));
                    apply(cx, vec![json!({"ev": "subagent_rows", "rows": done})]);
                    // The rail's other sessions settle too: after the turn's
                    // end nothing in the window runs (P3: then 0 presents).
                    let _ = upd(cx, &|v, cx| {
                        let metas: Vec<SessionMeta> = v
                            .model
                            .sessions
                            .iter()
                            .map(|m| SessionMeta { busy: false, subagents_running: 0, workers: Default::default(), ..m.clone() })
                            .collect();
                        v.apply_batch(vec![Event::Sessions(metas)], cx);
                    });
                    sleep(2.0).await;
                    // The turn ends from RUNNING, the orbit on screen: READY
                    // hands the floor back (Handoff in down the bar, the
                    // settle pass across the top, polish.md P3).
                    let _ = upd(cx, &|v, _| {
                        v.model.lives[v.panes.cur.live].transcript.apply(&SessionEvent::Idle);
                        v.sync_list();
                    });
                    sleep(3.0).await;
                    out["life_s"] = json!(round(t_main.elapsed().as_secs_f64(), 2));
                    println!("{out}");
                    let _ = cx.update(|_, cx| cx.quit());
                    return;
                }
                // `SYNAPS_BENCH_CHIPS=N` (Q5 r2 evidence, Haseeb's chips): N
                // workers running over the composer; after 2 s their steps
                // change one by one every 0.4 s (short, long, longer than the
                // chip), their clocks tick; at 8.5 s the second is done and
                // the third fails. Every chip's bounds after each change, in
                // the JSON (`chips`): what a capture of it shows, that none
                // moves. (No cancel arms here: the bench has no daemon to
                // send one to; `a_chip_is_the_columns_share_whatever_it_says`
                // arms one.)
                if let Some(n) = std::env::var("SYNAPS_BENCH_CHIPS").ok().and_then(|s| s.parse::<u64>().ok()) {
                    let ts = || chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Nanos, true);
                    let apply = |cx: &mut AsyncWindowContext, evs: Vec<Value>| {
                        let ts = ts();
                        let _ = upd(cx, &|v, cx| {
                            v.apply_batch(evs.iter().cloned().filter_map(local_raw).map(|e| e.with_ts(ts.clone())).collect(), cx);
                            v.sync_list();
                        });
                    };
                    let names = ["scout", "trace-cart", "reviewer", "flake-statistics-collector", "bisect", "har-diff"];
                    let steps = [
                        "ls",
                        "reading crates/relay/src/upstream.rs",
                        "grep -rn \"waitForResponse\" e2e/",
                        "git status",
                        "cargo test -p relay --test refresh_race -- --nocapture",
                        "reading src/session.rs",
                    ];
                    let born = Instant::now();
                    let rows = |at: &[usize], status: &[&str]| -> Value {
                        let rows: Vec<Value> = (0..n as usize)
                            .map(|k| {
                                let secs = round(born.elapsed().as_secs_f64() + 30. * k as f64, 1);
                                let st = status.get(k).copied().unwrap_or("running");
                                let done = (st != "running").then_some(secs);
                                json!({"subagent_id": k + 1, "agent_name": names[k % names.len()], "status": st, "cancel_requested": false,
                                    "elapsed_secs": secs, "finished_elapsed": done, "step": steps[at[k] % steps.len()], "tools": at[k] + 1,
                                    "handle_id": format!("sa_{}", k + 1)})
                            })
                            .collect();
                        json!({"ev": "subagent_rows", "rows": rows})
                    };
                    let _ = upd(cx, &|v, _| {
                        v.model.local_session(v.panes.cur.live, "bench-chips");
                        v.model.lives[v.panes.cur.live].transcript.apply_history(&dated(synthetic::history(40, 3)));
                        v.sync_list();
                        v.panes.cur.list.scroll_to_end();
                    });
                    let mut evs: Vec<Value> = (0..n as usize)
                        .flat_map(|k| {
                            let res = json!({"handle_id": format!("sa_{}", k + 1), "agent_name": names[k % names.len()], "status": "running"});
                            [
                                json!({"ev": "stream", "event": {"kind": "llm", "llm": "tool_use", "tool_name": "subagent_start",
                                    "tool_id": format!("w{k}"), "input": {"agent": names[k % names.len()], "task": "Find the refresh race"}}}),
                                json!({"ev": "stream", "event": {"kind": "llm", "llm": "tool_result", "tool_id": format!("w{k}"),
                                    "result": res.to_string()}}),
                            ]
                        })
                        .collect();
                    let mut at: Vec<usize> = (0..n as usize).collect();
                    evs.push(rows(&at, &[]));
                    apply(cx, evs);
                    let chips = |cx: &mut AsyncWindowContext| -> Value {
                        view.update(cx, |v, _| {
                            let mut b: Vec<(u64, [f32; 4])> = v
                                .chip_bounds
                                .borrow()
                                .iter()
                                .map(|(id, b)| (*id, [b.origin.x.into(), b.origin.y.into(), b.size.width.into(), b.size.height.into()]))
                                .collect();
                            b.sort_by_key(|c| c.0);
                            json!(b)
                        })
                        .unwrap_or(Value::Null)
                    };
                    sleep(2.0).await;
                    let mut log = vec![json!({"t": 2.0, "what": "spawned", "chips": chips(cx)})];
                    let mut status: Vec<&str> = Vec::new();
                    for i in 0..20usize {
                        let k = i % n as usize;
                        at[k] += 1;
                        let what = if i == 16 && n >= 3 {
                            status = vec!["running", "completed", "failed"];
                            "done, failed".to_string()
                        } else {
                            format!("step {k}")
                        };
                        apply(cx, vec![rows(&at, &status)]);
                        sleep(0.4).await;
                        log.push(json!({"t": round(t_main.elapsed().as_secs_f64(), 2), "what": what, "chips": chips(cx)}));
                    }
                    sleep(2.0).await;
                    out["chips"] = json!(log);
                    out["total_s"] = json!(round(t_main.elapsed().as_secs_f64(), 2));
                    println!("{out}");
                    let _ = cx.update(|_, cx| cx.quit());
                    return;
                }
                // `SYNAPS_BENCH_LINK=N`: only the link lost and back (motion.md
                // §5.3), the scene a capture records, after N s: up 2 s, down
                // 4 s (grace, tear, drain, held), up 3 s (Scan, restore). Every
                // frame's drain is in the debug log (`synaps_desktop::link`).
                if let Some(after) = std::env::var("SYNAPS_BENCH_LINK").ok().and_then(|s| s.parse::<f64>().ok()) {
                    sleep(after).await;
                    let conn = |up: bool| {
                        move |v: &mut AppView, _: &mut Context<AppView>| {
                            v.synthetic = false;
                            v.model.conn =
                                if up { crate::model::ConnState::Connected("bench".into()) } else { crate::model::ConnState::Reconnecting };
                        }
                    };
                    let _ = upd(cx, &|v, _| {
                        v.model.local_session(v.panes.cur.live, "bench-link");
                        v.model.lives[v.panes.cur.live].transcript.apply_history(&dated(synthetic::history(40, 3)));
                        v.sync_list();
                        v.panes.cur.list.scroll_to_end();
                    });
                    let _ = upd(cx, &conn(true));
                    sleep(2.0).await;
                    let (t0, c0, f0) = (Instant::now(), cpu_ns(), counts(cx));
                    let _ = upd(cx, &conn(false));
                    sleep(4.0).await;
                    out["link_lost_4s"] = stats(cx, t0, Instant::now(), c0, f0);
                    let (t0, c0, f0) = (Instant::now(), cpu_ns(), counts(cx));
                    let _ = upd(cx, &conn(true));
                    sleep(3.0).await;
                    out["link_back_3s"] = stats(cx, t0, Instant::now(), c0, f0);
                    out["total_s"] = json!(round(t_main.elapsed().as_secs_f64(), 2));
                    println!("{out}");
                    let _ = cx.update(|_, cx| cx.quit());
                    return;
                }
                // `SYNAPS_BENCH_WARM=S`: only the right panel's cache countdown
                // (`crate::warm`), 10 s at rest each: a session whose cache
                // expires in S s with the panel open (one frame a second under
                // ten minutes), the same with the panel folded (none), one
                // expiring in 52 minutes (one a minute), and none known.
                if let Some(secs) = std::env::var("SYNAPS_BENCH_WARM").ok().and_then(|s| s.parse::<u64>().ok()) {
                    let warm = |secs: Option<u64>| {
                        move |v: &mut AppView, _: &mut Context<AppView>| {
                            let now = crate::warm::Moment::now();
                            let stats = &mut v.model.lives[v.panes.cur.live].stats;
                            stats.warm = secs.map(|_| crate::warm::Warm::from_status(&json!({"keepwarm": {"mode": "auto"}}), now));
                            stats.expiry = secs.map(|s| {
                                let c = json!({"state": "warm", "warm_for_secs": s, "proof_ttl_secs": 300, "requested_ttl_secs": 300});
                                crate::warm::Expiry::new(synaps_client::decode_query_reply(&c), now)
                            });
                        }
                    };
                    let fold = |folded: bool| {
                        move |v: &mut AppView, cx: &mut Context<AppView>| {
                            let mut p = v.panels;
                            p.right_collapsed = folded;
                            v.set_panels_pub(p, cx);
                        }
                    };
                    let _ = upd(cx, &|v, _| v.model.local_session(v.panes.cur.live, "bench-warm"));
                    let _ = upd(cx, &warm(Some(secs)));
                    sleep(1.5).await;
                    for (name, step) in [
                        ("warm_open_10s", None),
                        ("warm_folded_10s", Some(fold(true))),
                        ("warm_reopened_10s", Some(fold(false))),
                    ] {
                        if let Some(step) = step {
                            let _ = upd(cx, &step);
                            sleep(1.5).await;
                        }
                        let (t0, c0, f0) = (Instant::now(), cpu_ns(), counts(cx));
                        sleep(10.0).await;
                        out[name] = stats(cx, t0, Instant::now(), c0, f0);
                    }
                    for (name, secs) in [("warm_52m_open_10s", Some(3125)), ("no_warm_open_10s", None)] {
                        let _ = upd(cx, &warm(secs));
                        sleep(1.5).await;
                        let (t0, c0, f0) = (Instant::now(), cpu_ns(), counts(cx));
                        sleep(10.0).await;
                        out[name] = stats(cx, t0, Instant::now(), c0, f0);
                    }
                    out["total_s"] = json!(round(t_main.elapsed().as_secs_f64(), 2));
                    println!("{out}");
                    let _ = cx.update(|_, cx| cx.quit());
                    return;
                }
                // `SYNAPS_BENCH_WARM_STATES=S`: the right panel's cache section
                // in each state it shows, S s apiece, each announced on stdout
                // (`warm-state <name>`) for a still: warm under a minute, the
                // band, cold, no matching request, unknown (`—`: parked), a
                // daemon that can't say (`—`), none at all (no section), 5m
                // proved of a 1h asked.
                if let Some(hold) = std::env::var("SYNAPS_BENCH_WARM_STATES").ok().and_then(|s| s.parse::<f64>().ok()) {
                    let _ = upd(cx, &|v, _| v.model.local_session(v.panes.cur.live, "bench-warm"));
                    let kw = json!({"mode": "off", "session_pings": 0, "session_keepwarm_usd": 0.0});
                    let st = json!({"keepwarm": kw});
                    let pinged = json!({"pinned": true,
                        "keepwarm": {"mode": "auto", "session_pings": 3, "session_keepwarm_usd": 0.0123, "period": {"next_ping_in_secs": 240}}});
                    let states = [
                        ("under-a-minute", Some(st.clone()), Some(json!({"state": "warm", "warm_for_secs": 42, "proof_ttl_secs": 300})), true),
                        (
                            "band",
                            Some(st.clone()),
                            Some(json!({"state": "unknown", "reason": "may_have_expired", "lapse_by_secs": 2400, "proof_ttl_secs": 300,
                                "requested_ttl_secs": 3600})),
                            true,
                        ),
                        ("cold", Some(st.clone()), Some(json!({"state": "cold", "reason": "expired", "proof_ttl_secs": 300})), true),
                        ("no-match", Some(st.clone()), Some(json!({"state": "unknown", "reason": "no_matching_request"})), true),
                        ("unknown", Some(st.clone()), Some(json!({"state": "unknown", "reason": "parked"})), true),
                        ("cannot-tell", Some(st.clone()), None, false),
                        ("none", None, None, true),
                        (
                            "five-of-an-hour",
                            Some(pinged),
                            Some(json!({"state": "warm", "warm_for_secs": 268, "proof_ttl_secs": 300, "requested_ttl_secs": 3600,
                                "proof_cache_tokens": 20000, "next_ping_in_secs": 240})),
                            true,
                        ),
                    ];
                    for (name, st, answer, asked) in states {
                        let set = move |v: &mut AppView, _: &mut Context<AppView>| {
                            let now = crate::warm::Moment::now();
                            v.model.expiry_query = asked;
                            let stats = &mut v.model.lives[v.panes.cur.live].stats;
                            stats.warm = st.as_ref().map(|s| crate::warm::Warm::from_status(s, now));
                            stats.expiry = answer.as_ref().map(|c| crate::warm::Expiry::new(synaps_client::decode_query_reply(c), now));
                        };
                        let _ = upd(cx, &set);
                        sleep(0.8).await;
                        println!("warm-state {name}");
                        sleep(hold).await;
                    }
                    let _ = cx.update(|_, cx| cx.quit());
                    return;
                }
                // A session at rest: nothing runs. (The synthetic history
                // ends on a call with no result, which reads as running:
                // it is the beat's scene, not this one.)
                // `SYNAPS_BENCH_IDLE_SECS=N`: N s instead, over a finished
                // history (its tool rows and prose at rest: the finish
                // gate's idle with the transcript's rows drawn).
                let idle = std::env::var("SYNAPS_BENCH_IDLE_SECS").ok().and_then(|s| s.parse::<u64>().ok());
                let _ = upd(cx, &|v, _| {
                    v.model.local_session(v.panes.cur.live, "bench-motion-idle");
                    if idle.is_some() {
                        v.model.lives[v.panes.cur.live].transcript.apply_history(&dated(synthetic::history(40, 3)));
                        v.sync_list();
                        v.panes.cur.list.scroll_to_end();
                    }
                });
                sleep(1.0).await;
                // `SYNAPS_BENCH_PANES=N` (split panes on): N panes on it
                // (split.md §4's shapes), settled, then their memory.
                if let Some(n) = std::env::var("SYNAPS_BENCH_PANES").ok().and_then(|s| s.parse::<usize>().ok()).filter(|n| *n > 1) {
                    // The keys stay on the window's root, as the 1-pane scene
                    // has them: a split gives them to the new pane's composer
                    // (at its next frame), whose caret blinks (2 Hz) for 10 s,
                    // then rests on (`theme::CURSOR_BLINK`): the idle window
                    // opens ~1.9 s after the last split, so it holds ~8 s of
                    // blinks. `SYNAPS_BENCH_PANES_CARET=1`: it does.
                    let caret = std::env::var("SYNAPS_BENCH_PANES_CARET").as_deref() == Ok("1");
                    for step in crate::pane::shape(n) {
                        let _ = view.update_in(cx, |v, w, cx| {
                            if let Some(i) = step.0 {
                                v.focus_nth(i, cx);
                            }
                            v.split_pane(step.1, w, cx);
                            if !caret {
                                v.keys_to = None;
                            }
                            cx.notify();
                        });
                        sleep(0.4).await;
                    }
                    sleep(1.5).await;
                    out["panes"] = json!(view.update(cx, |v, _| v.panes.len()).unwrap_or(0));
                    out["mem_panes"] = mem::sample();
                } else if std::env::var("SYNAPS_BENCH_PANES_CARET").as_deref() == Ok("1") {
                    // One pane, `SYNAPS_BENCH_PANES_CARET=1`: its composer
                    // takes the keys (at its next frame), as a split's new
                    // pane's does, as long before the idle window.
                    let _ = view.update(cx, |v, cx| v.focus_composer(cx));
                    sleep(1.9).await;
                }
                let (t0, c0, f0) = (Instant::now(), cpu_ns(), counts(cx));
                sleep(idle.unwrap_or(10) as f64).await;
                out[format!("idle_{}s", idle.unwrap_or(10))] = stats(cx, t0, Instant::now(), c0, f0);
                // `SYNAPS_BENCH_IDLE_ONLY=1`: the scene ends there.
                if std::env::var("SYNAPS_BENCH_IDLE_ONLY").as_deref() == Ok("1") {
                    println!("{out}");
                    let _ = cx.update(|_, cx| cx.quit());
                    return;
                }
                // A tool runs: its glyph scans on the beat (8 Hz), the foot's too.
                let _ = upd(cx, &|v, _| {
                    v.bench_session("bench-motion");
                    v.model.lives[v.panes.cur.live].transcript.apply_history(&dated(synthetic::history(40, 3)));
                    v.sync_list();
                    v.panes.cur.list.set_follow_mode(FollowMode::Tail);
                    v.panes.cur.list.scroll_to_end();
                    let t = &mut v.model.lives[v.panes.cur.live].transcript;
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
                    v.panes.cur.list.scroll_to_end();
                });
                sleep(1.0).await;
                let (t0, c0, f0) = (Instant::now(), cpu_ns(), counts(cx));
                sleep(6.0).await;
                out["beat"] = stats(cx, t0, Instant::now(), c0, f0);
                // 24 more: 25 glyphs scan on the one beat.
                let _ = upd(cx, &|v, _| {
                    let t = &mut v.model.lives[v.panes.cur.live].transcript;
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
                    v.panes.cur.list.scroll_to_end();
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
                    v.model.lives[v.panes.cur.live].transcript = synaps_client::transcript::Transcript::new();
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
                    v.bench_session("bench");
                    let res = json!({"handle_id": "sa_1", "agent_name": "scout", "status": "running"}).to_string();
                    let worker = [
                        json!({"ev": "stream", "event": {"kind": "llm", "llm": "tool_use", "tool_name": "subagent_start", "tool_id": "w1",
                            "input": {"agent": "scout", "task": "Map the relay's upstream calls"}}}),
                        json!({"ev": "stream", "event": {"kind": "llm", "llm": "tool_result", "tool_id": "w1", "result": res}}),
                        json!({"ev": "subagent_rows", "rows": [{"subagent_id": 1, "agent_name": "scout", "status": "running",
                            "cancel_requested": false, "elapsed_secs": 3.0, "finished_elapsed": null, "step": "ls", "tools": 1}]}),
                    ];
                    v.apply_batch(worker.into_iter().filter_map(local_raw).collect(), cx);
                    v.model.lives[v.panes.cur.live].stats.subagents.reattached();
                    if let Ok(request) = serde_json::from_value(json!({
                        "id": 7, "kind": "confirm", "title": "Run a command?", "prompt": "Tool: bash\nInput that will run:\n$ cargo test"
                    })) {
                        v.model.lives[v.panes.cur.live].transcript.apply(&SessionEvent::Prompt { request });
                    }
                    v.sync_list();
                });
                sleep(4.0).await;
                let (t0, c0, f0) = (Instant::now(), cpu_ns(), counts(cx));
                sleep(30.0).await;
                out["idle_30s_prompt"] = stats(cx, t0, Instant::now(), c0, f0);
                out["idle_30s_prompt"]["demoted_worker"] =
                    json!(view.update(cx, |v, _| v.model.lives[v.panes.cur.live].stats.subagents.demoted(1)).unwrap_or(false));
                out["idle_30s_prompt"]["sites"] = json!(view.update(cx, |v, _| v.fx.sites_text()).unwrap_or_default());
                out["idle_30s_prompt"]["pane_rects"] = json!(view
                    .update(cx, |v, _| v.panes.iter().map(|p| format!("{:?}", p.rect.get())).collect::<Vec<_>>())
                    .unwrap_or_default());
                out["idle_30s_prompt"]["prompt_pending"] =
                    json!(view.update(cx, |v, _| !v.model.lives[v.panes.cur.live].transcript.pending_prompts().is_empty()).unwrap_or(false));
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
                // Split panes step 4 (split.md §10): a rail row dragged over
                // the panes for 3 s, one move a frame, on a path through
                // each pane's edges and centre (the preview gliding between
                // zones), then Esc. Real pointer input through the window,
                // never from inside an `AppView` update (`3fe2291`). Split
                // off, no row drags: nothing to measure.
                if view.update(cx, |v, _| v.split_on()).unwrap_or(false) {
                    let (x0, y0, w, h) = view
                        .update(cx, |v, _| {
                            let ((x0, y0), (w, h)) = (v.centre_at.get(), v.centre.get());
                            (x0, y0, w, h)
                        })
                        .unwrap_or_default();
                    // A session's row (a section's label has no drag).
                    let row = view
                        .update(cx, |v, _| {
                            let rail = &v.rail.rail;
                            rail.items.iter().enumerate().find_map(|(ix, it)| {
                                crate::chrome::row_key(rail, it)?;
                                let b = v.rail_list.bounds_for_item(ix)?;
                                Some(point(b.left() + px(60.), b.top() + px(12.)))
                            })
                        })
                        .ok()
                        .flatten()
                        .unwrap_or(point(rail.left() + px(60.), rail.top() + px(60.)));
                    let input = |cx: &mut AsyncWindowContext, ev: PlatformInput| {
                        let _ = cx.update(|window, cx| {
                            window.dispatch_event(ev, cx);
                        });
                    };
                    let held = Some(MouseButton::Left);
                    let none = Modifiers::default();
                    input(cx, PlatformInput::MouseMove(MouseMoveEvent { position: row, pressed_button: None, modifiers: none }));
                    input(cx, PlatformInput::MouseDown(MouseDownEvent { button: MouseButton::Left, position: row, modifiers: none, click_count: 1, first_mouse: false }));
                    input(cx, PlatformInput::MouseMove(MouseMoveEvent { position: point(row.x + px(12.), row.y + px(4.)), pressed_button: held, modifiers: none }));
                    sleep(0.3).await;
                    let dragging = view.update(cx, |v, _| v.dnd.on.is_some()).unwrap_or(false);
                    let moves = 180;
                    let mut at = row;
                    let (t0, c0, f0) = (Instant::now(), cpu_ns(), counts(cx));
                    for i in 0..moves {
                        let s = i as f32 * 0.016;
                        let fx = 0.5 + 0.47 * (std::f32::consts::TAU * s / 1.5).sin();
                        let fy = 0.5 + 0.45 * (std::f32::consts::TAU * s / 1.1).sin();
                        at = point(px(x0 + w * fx), px(y0 + h * fy));
                        input(cx, PlatformInput::MouseMove(MouseMoveEvent { position: at, pressed_button: held, modifiers: none }));
                        sleep(0.016).await;
                    }
                    sleep(0.3).await;
                    out["drag_dnd"] = stats(cx, t0, Instant::now(), c0, f0);
                    out["drag_dnd"]["moves"] = json!(moves);
                    // The drag was ours all along (its targets up).
                    out["drag_dnd"]["ours"] = json!(dragging);
                    let n = view.update(cx, |v, _| v.panes.len()).unwrap_or(0);
                    out["drag_dnd"]["panes"] = json!(n);
                    let esc = Keystroke::parse("escape").expect("a key");
                    input(cx, PlatformInput::KeyDown(KeyDownEvent { keystroke: esc.clone(), is_held: false, prefer_character_input: false }));
                    input(cx, PlatformInput::KeyUp(KeyUpEvent { keystroke: esc }));
                    input(cx, PlatformInput::MouseUp(MouseUpEvent { button: MouseButton::Left, position: at, modifiers: none, click_count: 1 }));
                    sleep(0.5).await;
                    // Esc ended it, and nothing dropped.
                    out["drag_dnd"]["ended"] = json!(view.update(cx, |v, _| v.dnd.on.is_none() && v.panes.len() == n).unwrap_or(false));
                }
                // polish.md §6 gate 4: 5 sessions × 5 workers, 30 s. Five
                // busy sessions in the rail, each with five workers; the
                // shown one's five in its tray and transcript, running.
                let load_at = Instant::now();
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
                    v.panes.cur.list.scroll_to_end();
                });
                sleep(2.0).await;
                let (t0, c0, f0) = (Instant::now(), cpu_ns(), counts(cx));
                sleep(30.0).await;
                out["load_5x5_30s"] = stats(cx, t0, Instant::now(), c0, f0);
                out["load_5x5_30s"]["workers_running"] =
                    json!(view.update(cx, |v, _| v.model.lives[v.panes.cur.live].stats.subagents.running_count()).unwrap_or(0));
                out["load_5x5_30s"]["sites"] = json!(view.update(cx, |v, _| v.fx.site_count()).unwrap_or(0));
                out["load_5x5_30s"]["indicators"] = json!(view.update(cx, |v, _| v.fx.lattice_count()).unwrap_or(0));
                // Gate 5's sample after the 5 × 5, where main's run ends
                // (its `mem_end`): the cases after it are Q3's own.
                out["mem_after_load_5x5"] = mem::sample();
                // Q3 (polish.md §6 gate 4, the worst case): the same, and the
                // shown session's turn runs: the composer's orbit (Cards D
                // retired P2's sweep) over the 25 spinners. Gate 3's prompt is
                // answered first (NEEDS YOU outranks RUNNING: no orbit).
                let _ = upd(cx, &|v, _| {
                    let t = &mut v.model.lives[v.panes.cur.live].transcript;
                    t.apply(&SessionEvent::PromptResolved { prompt_id: 7 });
                    t.apply(&SessionEvent::TurnStarted { turn_baseline: 12, trigger: "user".into(), user_text: Some("Go on.".into()) });
                    t.apply(&SessionEvent::Stream { event: StreamEvent::Llm(LlmEvent::ResponseStart) });
                    v.sync_list();
                    v.panes.cur.list.scroll_to_end();
                });
                sleep(2.0).await;
                let (t0, c0, f0) = (Instant::now(), cpu_ns(), counts(cx));
                sleep(30.0).await;
                out["load_5x5_sweep_30s"] = stats(cx, t0, Instant::now(), c0, f0);
                out["load_5x5_sweep_30s"]["sites"] = json!(view.update(cx, |v, _| v.fx.site_count()).unwrap_or(0));
                out["load_5x5_sweep_30s"]["indicators"] = json!(view.update(cx, |v, _| v.fx.lattice_count()).unwrap_or(0));
                out["load_5x5_sweep_30s"]["state"] = json!(view.update(cx, |v, _| {
                    v.model.tabs.current().map(|t| format!("{:?}", v.tab_state(t))).unwrap_or_default()
                }).unwrap_or_default());
                // Shady #17 R2-M1 (gate 4 with P5 rolls from a live fan-out):
                // the same, and the shown session's five workers each call a
                // tool every 1.5 s, staggered (one every 0.3 s): its tool
                // count and step change (and roll). Then the same with the
                // step held, only the count changing. Then the first again
                // under Reduced (Shady #17 R3-M1): no orbit, no spinners, the
                // same data frames; Full minus it is what the motion costs.
                let steps = ["reading src/relay.rs", "grep -n rotate src", "reading src/session.rs", "cargo check -p relay", "reading src/store.rs"];
                let mut tools = [2u64; 5];
                let cases = [
                    ("load_5x5_fanout_30s", true, crate::motion::Level::Full),
                    ("load_5x5_fanout_tools_30s", false, crate::motion::Level::Full),
                    ("load_5x5_fanout_reduced_30s", true, crate::motion::Level::Reduced),
                ];
                for (case, step_moves, lv) in cases {
                    level(cx, lv);
                    sleep(1.0).await;
                    let (t0, c0, f0) = (Instant::now(), cpu_ns(), counts(cx));
                    let mut changes = 0u64;
                    while t0.elapsed() < Duration::from_secs(30) {
                        tools[changes as usize % 5] += 1;
                        let secs = round(3.0 + load_at.elapsed().as_secs_f64(), 1);
                        let rows: Vec<Value> = (0..5usize)
                            .map(|j| {
                                let step = if step_moves { steps[tools[j] as usize % steps.len()] } else { "reading src" };
                                json!({"subagent_id": j as u64 + 2, "agent_name": format!("worker-{}", j + 2), "status": "running",
                                    "cancel_requested": false, "elapsed_secs": secs, "finished_elapsed": null, "step": step, "tools": tools[j]})
                            })
                            .collect();
                        let _ = upd(cx, &|v, cx| {
                            v.apply_batch(local_raw(json!({"ev": "subagent_rows", "rows": rows.clone()})).into_iter().collect(), cx);
                            v.sync_list();
                        });
                        changes += 1;
                        sleep(0.3).await;
                    }
                    out[case] = stats(cx, t0, Instant::now(), c0, f0);
                    out[case]["changes"] = json!(changes);
                    out[case]["indicators"] = json!(view.update(cx, |v, _| v.fx.lattice_count()).unwrap_or(0));
                    out[case]["sites"] = json!(view.update(cx, |v, _| v.fx.site_count()).unwrap_or(0));
                }
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
                    v.model.local_session(v.panes.cur.live, "diag");
                    v.model.lives[v.panes.cur.live].transcript.apply_history(&dated(synthetic::history(opts.history_messages, 11)));
                    v.sync_list();
                    v.panes.cur.list.set_follow_mode(FollowMode::Normal);
                    v.panes.cur.list.scroll_to(ListOffset { item_ix: 0, offset_in_item: px(0.) });
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
                    v.panes.cur.list.scroll_to(ListOffset { item_ix: 0, offset_in_item: px(0.) });
                    v.bench.as_mut().unwrap().scroll = Some((Instant::now(), opts.scroll_px_per_s));
                });
                let t0 = Instant::now();
                sleep(2.5).await;
                out["scroll_markdown_from_top_again"] = stats(cx, t0, Instant::now());
                let _ = upd(cx, &|v, _| v.bench.as_mut().unwrap().scroll = None);
                for (name, plain, seed) in [("stream_plain", true, 21u64), ("stream_markdown", false, 22)] {
                    let _ = upd(cx, &move |v, _| {
                        v.plain_rows = plain;
                        v.model.local_session(v.panes.cur.live, "diag-stream");
                        v.reset_transcript();
                        v.model.lives[v.panes.cur.live].transcript.apply_history(&dated(synthetic::history(40, 3)));
                        v.model.lives[v.panes.cur.live].transcript.apply(&SessionEvent::Stream { event: StreamEvent::Llm(LlmEvent::ResponseStart) });
                        v.sync_list();
                        v.panes.cur.list.set_follow_mode(FollowMode::Tail);
                        v.panes.cur.list.scroll_to_end();
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
                v.model.local_session(v.panes.cur.live, "bench-stream");
                v.model.lives[v.panes.cur.live].transcript.apply_history(&dated(synthetic::history(40, 3)));
                v.model.lives[v.panes.cur.live].transcript.apply(&SessionEvent::TurnStarted {
                    turn_baseline: 40,
                    trigger: "user".into(),
                    user_text: Some("Explain the frame pipeline, with code.".into()),
                });
                v.model.lives[v.panes.cur.live].transcript.apply(&SessionEvent::Stream { event: StreamEvent::Llm(LlmEvent::ResponseStart) });
                v.sync_list();
                v.panes.cur.list.set_follow_mode(FollowMode::Tail);
                v.panes.cur.list.scroll_to_end();
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
                    s["streamed_chars"] = json!(v.model.lives[v.panes.cur.live].transcript.items().last().map(|i| i.text.len()));
                    s
                })
                .unwrap_or(Value::Null);
            out["stream"] = stream;
            out["mem_after_stream"] = mem::sample();

            // 3. A 2,000-message transcript with code blocks, scrolled top → down.
            let _ = upd(cx, &|v, _| {
                v.model.local_session(v.panes.cur.live, "bench-long");
                v.reset_transcript();
                v.model.lives[v.panes.cur.live].transcript.apply_history(&dated(synthetic::history(opts.history_messages, 11)));
                v.sync_list();
                v.panes.cur.list.set_follow_mode(FollowMode::Normal);
                v.panes.cur.list.scroll_to(ListOffset { item_ix: 0, offset_in_item: px(0.) });
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
                    s["rows"] = json!(v.model.lives[v.panes.cur.live].transcript.len());
                    s["messages"] = json!(opts.history_messages);
                    s["px_per_s"] = json!(speed);
                    s["reached_row"] = json!(v.panes.cur.list.logical_scroll_top().item_ix);
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
            let center = view.update(cx, |v, _| v.panes.cur.list.viewport_bounds().center()).unwrap_or_default();
            for (name, level) in [("spin_full", crate::motion::Level::Full), ("spin_reduced", crate::motion::Level::Reduced)] {
                let _ = cx.update(|_, cx| {
                    cx.set_global(crate::motion::Setting { level: Some(level), lock: None });
                    crate::motion::Level::apply(cx);
                });
                let _ = upd(cx, &|v, _| v.bench.as_mut().unwrap().animate = false);
                sleep(0.3).await;
                let t0 = Instant::now();
                let start = view.update(cx, |v, _| -f32::from(v.panes.cur.list.scroll_px_offset_for_scrollbar().y)).unwrap_or(0.);
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
                        s["travel_px"] = json!(round((-f32::from(v.panes.cur.list.scroll_px_offset_for_scrollbar().y) - start) as f64, 1));
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
                v.model.local_session(v.panes.cur.live, "bench-drag");
                v.reset_transcript();
                v.model.lives[v.panes.cur.live].transcript.apply_history(&turns(DRAG_TURNS, 13));
                v.sync_list();
                v.panes.cur.list.set_follow_mode(FollowMode::Tail);
                v.panes.cur.list.scroll_to_end();
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
                        s["rows"] = json!(v.model.lives[v.panes.cur.live].transcript.len());
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
                        v.model.want(v.panes.cur.live, seq);
                        v.bench.as_mut().unwrap().attach_seq = Some(seq);
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
                        if let Some(seq) = v.bench.as_ref().unwrap().attach_seq {
                            let _ = v.client.as_ref().unwrap().on(seq).submit(p.clone());
                        }
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
                                "rows": v.model.lives[v.panes.cur.live].transcript.len(),
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
