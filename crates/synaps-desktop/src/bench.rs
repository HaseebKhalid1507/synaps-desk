//! `--bench`: self-driving measurements, printed as one JSON object, then
//! exit. Synthetic load comes from `synaps_client::synthetic` and flows
//! through the same event path as daemon traffic (channel → pending →
//! once-per-frame apply). Frame data comes from GPUI's own profiler
//! (`FrameEvent::Draw` / `Present`).

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui_kit::profiler::{FrameEvent, FrameTimingCollector};
use gpui_kit::*;
use serde_json::{Value, json};
use synaps_client::protocol::{LlmEvent, SessionEvent, SessionStreamEvent, StreamEvent};
use synaps_client::{Event, synthetic};

use crate::app::AppView;
use crate::mem;

pub struct BenchOptions {
    pub t_main: Instant,
    pub exec_to_main_ms: Option<f64>,
    pub refresh_hz: Option<f64>,
    pub prompt: Option<String>,
    pub stream_secs: f64,
    pub scroll_secs: f64,
    pub scroll_px_per_s: f32,
    pub history_messages: usize,
    pub diag: bool,
}

pub struct BenchState {
    collector: FrameTimingCollector,
    pub frames: Vec<FrameEvent>,
    pub scroll: Option<(Instant, f32)>,
    pub animate: bool,
    pub submit_at: Option<Instant>,
    turn_seen: bool,
    pub first_text_at: Option<Instant>,
    pub idle_at: Option<Instant>,
    pub attached_at: Option<Instant>,
    pub received_deltas: usize,
}

impl BenchState {
    pub fn new() -> Self {
        Self {
            collector: FrameTimingCollector::new(),
            frames: Vec::new(),
            scroll: None,
            animate: false,
            submit_at: None,
            turn_seen: false,
            first_text_at: None,
            idle_at: None,
            attached_at: None,
            received_deltas: 0,
        }
    }

    pub fn on_received(&mut self, batch: &[Event], now: Instant) {
        for e in batch {
            match e {
                Event::Attached { .. } => {
                    self.attached_at.get_or_insert(now);
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

    fn drain(&mut self) {
        self.frames.extend(self.collector.collect_unseen());
    }

    fn first_present_after(&self, t: Instant) -> Option<Instant> {
        self.frames.iter().find_map(|f| match f {
            FrameEvent::Present(p) if p.present_end >= t => Some(p.present_end),
            _ => None,
        })
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
        let refresh_source = if self.refresh().is_some() { "gpui" } else if refresh_fallback.is_some() { "--refresh-hz" } else { "unknown" };
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
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(30));
        eprintln!("bench: watchdog fired at 30 s");
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
                out["exec_to_first_frame_ms_approx"] =
                    json!(opts.exec_to_main_ms.map(|e| round(e + ms(f.duration_since(t_main)), 1)));
            }
            sleep(1.5).await;
            out["mem_idle"] = mem::sample();
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
                    v.active = Some("diag".into());
                    v.transcript.apply_history(&synthetic::history(opts.history_messages, 11));
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
                        v.active = Some("diag-stream".into());
                        v.transcript = synaps_client::Transcript::new();
                        v.sync_list();
                        v.transcript.apply_history(&synthetic::history(40, 3));
                        v.transcript.apply(&SessionEvent::Stream { event: StreamEvent::Llm(LlmEvent::ResponseStart) });
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
                            let _ = ftx.try_send(Event::Session { session_id: "diag-stream".into(), seq: i as u64, event: ev });
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
                v.active = Some("bench-stream".into());
                v.transcript.apply_history(&synthetic::history(40, 3));
                v.transcript.apply(&SessionEvent::TurnStarted {
                    turn_baseline: 40,
                    trigger: "user".into(),
                    user_text: Some("Explain the frame pipeline, with code.".into()),
                });
                v.transcript.apply(&SessionEvent::Stream { event: StreamEvent::Llm(LlmEvent::ResponseStart) });
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
                    let _ = feeder_tx.try_send(Event::Session { session_id: "bench-stream".into(), seq: i as u64, event: ev });
                }
                start.elapsed()
            });
            sleep(opts.stream_secs + 0.1).await;
            let t1 = Instant::now();
            let fed = feeder.join().unwrap_or_default();
            for ev in [
                SessionEvent::Stream { event: StreamEvent::Session(SessionStreamEvent::Done) },
                SessionEvent::Idle,
            ] {
                let _ = tx.try_send(Event::Session { session_id: "bench-stream".into(), seq: 0, event: ev });
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
                    s["streamed_chars"] = json!(v.transcript.items.last().map(|i| i.text.len()));
                    s
                })
                .unwrap_or(Value::Null);
            out["stream"] = stream;
            out["mem_after_stream"] = mem::sample();

            // 3. A 2,000-message transcript with code blocks, scrolled top → down.
            let _ = upd(cx, &|v, _| {
                v.active = Some("bench-long".into());
                v.transcript = synaps_client::Transcript::new();
                v.sync_list();
                v.transcript.apply_history(&synthetic::history(opts.history_messages, 11));
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
                    s["rows"] = json!(v.transcript.len());
                    s["messages"] = json!(opts.history_messages);
                    s["px_per_s"] = json!(speed);
                    s["reached_row"] = json!(v.list.logical_scroll_top().item_ix);
                    s
                })
                .unwrap_or(Value::Null);
            out["scroll"] = scroll;
            sleep(0.3).await;
            out["mem_after_scroll"] = mem::sample();

            // 4. One real turn against the daemon, if connected.
            let has_client = view.update(cx, |v, _| v.client.is_some()).unwrap_or(false);
            if let (true, Some(prompt)) = (has_client, opts.prompt.clone()) {
                let t_create = Instant::now();
                let _ = upd(cx, &|v, _| {
                    v.client.as_ref().unwrap().create(None);
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
                        v.client.as_ref().unwrap().submit(p.clone());
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
                                "rows": v.transcript.len(),
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
