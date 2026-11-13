//! The avatar view: paints the current frame, the HUD (status tag, speech bubble, menu,
//! headpat hearts), handles pointer input, and keeps the X input/bounding shape in step
//! with what is drawn. All state changes go through `apply` (commands) or pointer handlers.

use crate::anim::{Animator, Mood, Rng, Seq};
use crate::control::Command;
use crate::frames::{Frame, Geometry};
use crate::pack::{MoodSpec, Pack};
use crate::platform::x11::X11;
use crate::shape::{self, ShapeRect};
use crate::ticker::Ticker;
use crate::state;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Window layout in device pixels (X11 works in these; GPUI gets them / scale factor).
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub sf: f32,
    pub geo: Geometry,
    pub win_w: i32,
    pub win_h: i32,
    pub avatar_x: i32,
    pub avatar_y: i32,
    /// Horizontal centre of the character (canonical anchor x).
    pub face_x: i32,
    pub tag_bottom: i32,
    pub bubble_bottom: i32,
    /// Ground point (the feet) inside the window.
    pub ground_x: i32,
    pub ground_y: i32,
}

// Logical px. The HUD numbers follow Capri's own CSS (cyberpunk.css, avatar-detach.css).
const MARGIN: f32 = 8.0;
const TOP_MARGIN: f32 = 6.0;
const BODY_FONT: f32 = 18.0;
const BODY_LINE: f32 = 24.0;
const BODY_MAX_H: f32 = 5.0 * BODY_LINE;
/// Padding 12 + 14, title 12 + gap 5, body.
const BUBBLE_MAX_H: f32 = 26.0 + 17.0 + BODY_MAX_H;
const BUBBLE_MAX_W: f32 = 460.0;
const BUBBLE_GAP: f32 = 9.0;
const TAG_FONT: f32 = 10.0;
/// Font line + padding 4 + 4 + border 1 + 1.
const TAG_H: f32 = TAG_FONT + 1.0 + 10.0;
const TAG_GAP: f32 = 4.0;
const MIN_WIDTH: f32 = 500.0;
const CUT: f32 = 12.0;
const OUTLINE: f32 = 2.0;
const TAIL_W: f32 = 22.0;
const TAIL_H: f32 = 14.0;

impl Layout {
    pub fn new(pack: &Pack, sf: f32, body_height: f32) -> Layout {
        let geo = Geometry::new(pack, body_height * sf);
        let d = |l: f32| (l * sf).round() as i32;
        let win_w = (geo.width as i32).max(d(MIN_WIDTH));
        let avatar_x = (win_w - geo.width as i32) / 2;
        // Top of the standing (default) frame inside the avatar rect.
        let default = &pack.frames[pack.frame(&pack.catalog.default_sprite).unwrap_or(0)];
        let (_, dy) = pack.offset(default);
        let (_, head) = geo.to_device(0, default.bounds.y + dy);
        let head_top = d(TOP_MARGIN + BUBBLE_MAX_H + BUBBLE_GAP + TAG_H + TAG_GAP);
        let avatar_y = head_top - head.round() as i32;
        let tag_bottom = head_top - d(TAG_GAP);
        let c = pack.catalog.canonical_anchor;
        let (gx, gy) = geo.to_device(c.x, c.y);
        Layout {
            sf,
            geo,
            win_w,
            win_h: avatar_y + geo.height as i32,
            avatar_x,
            avatar_y,
            face_x: avatar_x + gx.round() as i32,
            tag_bottom,
            bubble_bottom: tag_bottom - d(TAG_H) - d(BUBBLE_GAP),
            ground_x: avatar_x + gx.round() as i32,
            ground_y: avatar_y + gy.round() as i32,
        }
    }

    fn lp(&self, d: i32) -> Pixels {
        px(d as f32 / self.sf)
    }
}

struct Bubble {
    text: Vec<char>,
    shown: usize,
    per_tick: usize,
    next_char: Option<Instant>,
    hide_at: Option<Instant>,
    fade: f32,
}

const TYPE_TICK: Duration = Duration::from_millis(35);
const FADE_STEPS: u32 = 9;
const FADE_TICK: Duration = Duration::from_millis(50);
const HEARTS_MS: u64 = 1_250;

struct Press {
    root: (i32, i32),
    origin: (i32, i32),
    head: bool,
    dragging: bool,
}

pub struct Avatar {
    pack: Arc<Pack>,
    pub layout: Layout,
    x11: Arc<X11>,
    name: SharedString,
    font: SharedString,
    frames: Vec<Option<Frame>>,
    loaded: bool,
    anim: Animator,
    mood: Mood,
    mood_then: Option<(Instant, Mood)>,
    next_activity: Option<Instant>,
    autonomy_started: bool,
    rng: Rng,
    bubble: Option<Bubble>,
    hearts: Option<(Instant, Point<Pixels>)>,
    menu: Option<(Point<Pixels>, Instant)>,
    press: Option<Press>,
    streak: (u32, Instant),
    origin: (i32, i32),
    ticker: Ticker,
    _tick_loop: Task<()>,
    last_shape: Rc<RefCell<(Vec<ShapeRect>, Vec<ShapeRect>)>>,
    t_start: Instant,
    first_frame: Rc<Cell<bool>>,
    focus: FocusHandle,
    focus_probe: Option<String>,
    warmed: bool,
    /// Horizontal device-px range of the window that is on the avatar's monitor.
    vis: (i32, i32),
}

pub struct Init {
    pub pack: Arc<Pack>,
    pub layout: Layout,
    pub x11: Arc<X11>,
    pub name: String,
    pub origin: (i32, i32),
    pub t_start: Instant,
    pub focus_probe: bool,
}

impl Avatar {
    pub fn new(init: Init, cx: &mut Context<Self>) -> Avatar {
        let pack = init.pack;
        let default = pack.frame(&pack.catalog.default_sprite).unwrap_or(0);
        let font = pack.behaviour.font.clone().unwrap_or_else(|| "Fira Sans".into());
        let vis = Self::visible_range(&init.x11, &init.layout, init.origin);
        let (ticker, wake) = Ticker::start();
        let tick_loop = cx.spawn(async move |this, cx| {
            while wake.recv().await.is_ok() {
                if this.update(cx, |v, cx| v.tick(cx)).is_err() {
                    break;
                }
            }
        });
        Avatar {
            frames: (0..pack.frames.len()).map(|_| None).collect(),
            anim: Animator::new(default),
            name: init.name.to_uppercase().into(),
            font: font.into(),
            pack,
            layout: init.layout,
            x11: init.x11,
            loaded: false,
            mood: Mood::Idle,
            mood_then: None,
            next_activity: None,
            autonomy_started: false,
            rng: Rng::seeded(),
            bubble: None,
            hearts: None,
            menu: None,
            press: None,
            streak: (0, Instant::now()),
            origin: init.origin,
            ticker,
            _tick_loop: tick_loop,
            last_shape: Rc::new(RefCell::new((vec![(0, 0, 1, 1)], vec![]))),
            t_start: init.t_start,
            first_frame: Rc::new(Cell::new(false)),
            focus: cx.focus_handle(),
            focus_probe: init.focus_probe.then(String::new),
            warmed: false,
            vis,
        }
    }

    /// Frames arrive from the background decoder: the default first, then the rest.
    pub fn add_frames(&mut self, decoded: Vec<(usize, Frame)>, complete: bool, cx: &mut Context<Self>) {
        for (i, f) in decoded {
            self.frames[i] = Some(f);
        }
        if complete && !self.loaded {
            self.loaded = true;
            let now = Instant::now();
            self.set_mood(Mood::Idle, now);
        }
        cx.notify();
        self.schedule(cx);
    }

    fn spec(&self, m: Mood) -> Option<&MoodSpec> {
        self.pack.behaviour.moods.get(m.key())
    }

    fn frame_or_default(&self, i: usize) -> Option<&Frame> {
        self.frames.get(i).and_then(|f| f.as_ref()).or_else(|| {
            let d = self.pack.frame(&self.pack.catalog.default_sprite).ok()?;
            self.frames.get(d)?.as_ref()
        })
    }

    fn loop_seq(&self, clip: Option<&str>) -> Option<Seq> {
        let c = self.pack.clip(clip?)?;
        Some(c.frames.iter().filter_map(|f| Some((*self.pack.index.get(&f.id)?, f.ms))).collect())
    }

    fn autonomy_gap(&mut self) -> Duration {
        let a = &self.pack.catalog.autonomy;
        let base = if self.autonomy_started { a.activity_gap_ms } else { a.first_activity_ms };
        Duration::from_millis((base as f32 * self.rng.range(0.75, 1.35)) as u64)
    }

    pub fn set_mood(&mut self, m: Mood, now: Instant) {
        let default = self.pack.frame(&self.pack.catalog.default_sprite).unwrap_or(0);
        let mut seq: Seq = Vec::new();
        if m != self.mood
            && let Some(exit) = self.spec(self.mood).and_then(|s| s.exit.clone())
        {
            seq.extend(self.pack.flatten(&exit));
        }
        let spec = self.spec(m).cloned();
        if let Some(entry) = spec.as_ref().and_then(|s| s.entry.clone()) {
            seq.extend(self.pack.flatten(&entry));
        }
        let base = spec.as_ref().and_then(|s| self.pack.index.get(&s.base).copied()).unwrap_or(default);
        let looping = self.loop_seq(spec.as_ref().and_then(|s| s.r#loop.as_deref()));
        self.anim.set_layer(base, looping, now);
        if !seq.is_empty() {
            self.anim.play(seq, now);
        } else if m != self.mood {
            self.anim.stop_oneshot(now);
        }
        self.mood = m;
        self.mood_then = spec
            .as_ref()
            .and_then(|s| s.then.as_ref())
            .and_then(|t| Some((now + Duration::from_millis(t.after_ms), Mood::parse(&t.mood)?)));
        let autonomous = spec.as_ref().map_or(m == Mood::Idle, |s| s.autonomy);
        self.next_activity = if autonomous && self.pack.catalog.autonomy.enabled && !self.pack.behaviour.activities.is_empty() {
            Some(now + self.autonomy_gap())
        } else {
            None
        };
    }

    fn status(&self) -> (String, u32) {
        let spec = self.spec(self.mood);
        let label = spec.and_then(|s| s.status.clone()).unwrap_or_else(|| self.mood.default_status().into());
        (label, self.pack.tone(spec.and_then(|s| s.tone.as_deref())))
    }

    // ---- timing -------------------------------------------------------------------

    fn next_deadline(&self) -> Option<Instant> {
        let mut t: Option<Instant> = None;
        let mut take = |d: Option<Instant>| {
            if let Some(d) = d {
                t = Some(t.map_or(d, |t| t.min(d)));
            }
        };
        take(self.anim.deadline());
        if !self.anim.busy() {
            take(self.next_activity);
        }
        take(self.mood_then.map(|m| m.0));
        if let Some(b) = &self.bubble {
            take(b.next_char);
            take(b.hide_at);
        }
        if let Some((t0, _)) = self.hearts {
            take(Some((Instant::now() + Duration::from_millis(33)).min(t0 + Duration::from_millis(HEARTS_MS))));
        }
        take(self.menu.map(|m| m.1));
        t
    }

    fn schedule(&mut self, _cx: &mut Context<Self>) {
        self.ticker.set(self.next_deadline());
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        if std::env::var_os("SA_DEBUG").is_some() {
            eprintln!("tick at {} ms", self.t_start.elapsed().as_millis());
        }
        self.anim.tick(now);
        if let Some((at, m)) = self.mood_then
            && at <= now
        {
            self.set_mood(m, now);
        }
        if let Some(at) = self.next_activity
            && at <= now
            && !self.anim.busy()
            && !self.pack.behaviour.activities.is_empty()
        {
            let i = self.rng.pick(self.pack.behaviour.activities.len());
            let seq = self.pack.flatten(&self.pack.behaviour.activities[i]);
            let len: u64 = seq.iter().map(|s| s.1 as u64).sum();
            self.anim.play(seq, now);
            self.autonomy_started = true;
            self.next_activity = Some(now + Duration::from_millis(len) + self.autonomy_gap());
        }
        let mut typed_out = false;
        if let Some(b) = &mut self.bubble {
            if let Some(at) = b.next_char
                && at <= now
            {
                b.shown = (b.shown + b.per_tick).min(b.text.len());
                if b.shown == b.text.len() {
                    b.next_char = None;
                    typed_out = true;
                    let hold = (2_500 + 30 * b.text.len() as u64).min(9_000);
                    b.hide_at = Some(now + Duration::from_millis(hold));
                } else {
                    b.next_char = Some(now + TYPE_TICK);
                }
            }
            if let Some(at) = b.hide_at
                && at <= now
            {
                // No compositor, no alpha: skip the fade rather than dim to black.
                b.fade = if self.x11.composited() { b.fade - 1.0 / FADE_STEPS as f32 } else { 0.0 };
                b.hide_at = Some(now + FADE_TICK);
            }
            if b.fade <= 0.01 {
                self.bubble = None;
            }
        }
        // Keep SPEAKING up a beat after the last word, then settle.
        if typed_out && self.mood == Mood::Talking {
            self.mood_then = Some((now + Duration::from_millis(1_400), Mood::Idle));
        }
        if let Some((t0, _)) = self.hearts
            && now.duration_since(t0) >= Duration::from_millis(HEARTS_MS)
        {
            self.hearts = None;
        }
        if let Some((_, until)) = self.menu
            && until <= now
        {
            self.menu = None;
        }
        cx.notify();
        self.schedule(cx);
    }

    // ---- commands -------------------------------------------------------------------

    pub fn apply(&mut self, cmd: Command, cx: &mut Context<Self>) -> String {
        let now = Instant::now();
        let reply = match cmd {
            Command::Mood(m) => {
                self.set_mood(m, now);
                "ok".into()
            }
            Command::Clip(id) => {
                let steps = [crate::pack::Step::Clip(id.clone())];
                let seq = self.pack.flatten(&steps);
                if seq.is_empty() {
                    match self.pack.frame(&id) {
                        Ok(i) => {
                            self.anim.play(vec![(i, 1_500)], now);
                            "ok (frame)".into()
                        }
                        Err(_) => format!("error: no clip, action or frame `{id}`"),
                    }
                } else {
                    self.anim.play(seq, now);
                    "ok".into()
                }
            }
            Command::Say(text) => {
                self.say(text, now);
                "ok".into()
            }
            Command::Pos(x, y) => {
                let o = (x - self.layout.ground_x, y - self.layout.ground_y);
                self.move_to(o, true);
                let (gx, gy) = self.ground();
                format!("ok {gx} {gy}")
            }
            Command::State => self.state_json(),
            Command::Quit => {
                self.quit(cx);
                "bye".into()
            }
            Command::Reshape => {
                self.last_shape.borrow_mut().0.clear();
                "ok".into()
            }
        };
        if let Some(t) = self.next_activity
            && t < now + Duration::from_secs(4)
        {
            self.next_activity = Some(now + Duration::from_secs(4));
        }
        cx.notify();
        self.schedule(cx);
        reply
    }

    fn say(&mut self, text: String, now: Instant) {
        let text: Vec<char> = text.chars().collect();
        // Reveal in at most ~5 s however long the text is.
        let per_tick = (text.len() / (5_000 / TYPE_TICK.as_millis() as usize)).max(2);
        self.bubble = Some(Bubble { text, shown: 0, per_tick, next_char: Some(now), hide_at: None, fade: 1.0 });
        if self.mood != Mood::Talking {
            self.set_mood(Mood::Talking, now);
        } else {
            self.mood_then = None;
        }
    }

    fn quit(&mut self, cx: &mut Context<Self>) {
        state::save(self.ground());
        std::fs::remove_file(crate::control::socket_path()).ok();
        cx.quit();
    }

    fn state_json(&self) -> String {
        let frame = self.frame_or_default(self.anim.frame()).map(|f| f.id.clone()).unwrap_or_default();
        let (gx, gy) = self.ground();
        let m = self.x11.monitor_at(gx, gy - 1);
        serde_json::json!({
            "mood": self.mood.key(),
            "monitor": { "x": m.x, "y": m.y, "w": m.w, "h": m.h },
            "frame": frame,
            "status": self.status().0,
            "window": { "x": self.origin.0, "y": self.origin.1, "w": self.layout.win_w, "h": self.layout.win_h },
            "ground": [gx, gy],
            "avatar": { "x": self.origin.0 + self.layout.avatar_x, "y": self.origin.1 + self.layout.avatar_y,
                        "w": self.layout.geo.width, "h": self.layout.geo.height },
            "bubble": self.bubble.as_ref().map(|b| b.text.iter().collect::<String>()),
            "composited": self.x11.composited(),
            "loaded": self.loaded,
            "scale_factor": self.layout.sf,
            "focus_probe": self.focus_probe,
        })
        .to_string()
    }

    // ---- placement -------------------------------------------------------------------

    pub fn ground(&self) -> (i32, i32) {
        (self.origin.0 + self.layout.ground_x, self.origin.1 + self.layout.ground_y)
    }

    /// Keep the avatar (not the bubble area) on the monitor under it; feet no lower than
    /// the monitor's bottom edge.
    pub fn clamp(x11: &X11, l: &Layout, (x, y): (i32, i32)) -> (i32, i32) {
        let (aw, ah) = (l.geo.width as i32, l.geo.height as i32);
        let (cx, cy) = (x + l.avatar_x + aw / 2, y + l.avatar_y + ah / 2);
        let m = x11.monitor_at(cx, cy);
        let ax = (x + l.avatar_x).clamp(m.x, (m.x + m.w - aw).max(m.x));
        let gy = (y + l.ground_y).min(m.y + m.h);
        let top = gy - l.ground_y + l.avatar_y;
        let gy = if top < m.y { gy + (m.y - top) } else { gy };
        (ax - l.avatar_x, gy - l.ground_y)
    }

    pub fn default_origin(x11: &X11, l: &Layout) -> (i32, i32) {
        let m = x11.primary();
        let x = m.x + m.w - (l.avatar_x + l.geo.width as i32) - (24.0 * l.sf) as i32;
        let y = m.y + m.h - l.ground_y;
        (x, y)
    }

    /// Which part of the window is on the avatar's monitor; the bubble stays inside it.
    fn visible_range(x11: &X11, l: &Layout, o: (i32, i32)) -> (i32, i32) {
        let m = x11.monitor_at(o.0 + l.avatar_x + l.geo.width as i32 / 2, o.1 + l.avatar_y + l.geo.height as i32 / 2);
        ((m.x - o.0).max(0), (m.x + m.w - o.0).min(l.win_w))
    }

    fn move_to(&mut self, o: (i32, i32), persist: bool) {
        let o = Self::clamp(&self.x11, &self.layout, o);
        if o != self.origin {
            self.origin = o;
            self.x11.move_to(o.0, o.1).ok();
        }
        self.vis = Self::visible_range(&self.x11, &self.layout, o);
        if persist {
            state::save(self.ground());
        }
    }

    // ---- pointer --------------------------------------------------------------------

    fn local(&self, p: Point<Pixels>) -> (i32, i32) {
        let sf = self.layout.sf;
        ((f32::from(p.x) * sf) as i32 - self.layout.avatar_x, (f32::from(p.y) * sf) as i32 - self.layout.avatar_y)
    }

    fn hit(&self, p: Point<Pixels>) -> Option<bool> {
        let f = self.frame_or_default(self.anim.frame())?;
        let (x, y) = self.local(p);
        f.mask.get(x, y).then_some(y < f.head_line)
    }

    fn mouse_down(&mut self, ev: &MouseDownEvent, cx: &mut Context<Self>) {
        if self.menu.take().is_some() {
            cx.notify();
            self.schedule(cx);
        }
        let Some(head) = self.hit(ev.position) else { return };
        match ev.button {
            MouseButton::Left => {
                let root = self.x11.pointer().unwrap_or((0, 0));
                self.press = Some(Press { root, origin: self.origin, head, dragging: false });
            }
            MouseButton::Right => {
                self.menu = Some((ev.position, Instant::now() + Duration::from_secs(8)));
                cx.notify();
                self.schedule(cx);
            }
            _ => {}
        }
    }

    fn mouse_move(&mut self, ev: &MouseMoveEvent, _cx: &mut Context<Self>) {
        if ev.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let Some(p) = &mut self.press else { return };
        let Ok(root) = self.x11.pointer() else { return };
        let (dx, dy) = (root.0 - p.root.0, root.1 - p.root.1);
        if !p.dragging && dx * dx + dy * dy < 25 {
            return;
        }
        p.dragging = true;
        let o = (p.origin.0 + dx, p.origin.1 + dy);
        self.move_to(o, false);
    }

    fn mouse_up(&mut self, ev: &MouseUpEvent, cx: &mut Context<Self>) {
        if ev.button != MouseButton::Left {
            return;
        }
        let Some(p) = self.press.take() else { return };
        if p.dragging {
            state::save(self.ground());
            return;
        }
        let now = Instant::now();
        self.streak = if now.duration_since(self.streak.1) < Duration::from_millis(self.pack.behaviour.click.streak_ms) {
            (self.streak.0 + 1, now)
        } else {
            (1, now)
        };
        let click = self.pack.behaviour.click.clone();
        let steps = if self.streak.0 >= click.annoyed_after && !click.annoyed.is_empty() {
            self.streak.0 = 0;
            click.annoyed
        } else if p.head && !click.head.is_empty() {
            self.hearts = Some((now, ev.position));
            click.head
        } else if !click.body.is_empty() {
            let i = self.rng.pick(click.body.len());
            click.body[i].clone()
        } else {
            Vec::new()
        };
        let seq = self.pack.flatten(&steps);
        self.anim.play(seq, now);
        if self.next_activity.is_some() {
            self.next_activity = Some(now + self.autonomy_gap());
        }
        cx.notify();
        self.schedule(cx);
    }

    fn reset_position(&mut self, cx: &mut Context<Self>) {
        self.menu = None;
        let o = Self::default_origin(&self.x11, &self.layout);
        self.move_to(o, true);
        cx.notify();
        self.schedule(cx);
    }
}

type Cell2 = Rc<Cell<Option<Bounds<Pixels>>>>;

/// Wraps `child` in a plain box whose laid-out bounds (= the child's border box) land
/// in `cell` during prepaint, for the X shape.
fn probed(child: impl IntoElement, cell: &Cell2) -> Div {
    let cell = cell.clone();
    div()
        .relative()
        .child(child)
        .child(canvas(move |b, _, _| cell.set(Some(b)), |_, _, _, _| {}).absolute().top_0().left_0().size_full())
}

fn to_device(b: Bounds<Pixels>, sf: f32) -> ShapeRect {
    let x0 = (f32::from(b.origin.x) * sf).floor();
    let y0 = (f32::from(b.origin.y) * sf).floor();
    let x1 = (f32::from(b.origin.x + b.size.width) * sf).ceil();
    let y1 = (f32::from(b.origin.y + b.size.height) * sf).ceil();
    (x0 as i16, y0 as i16, (x1 - x0).max(1.0) as u16, (y1 - y0).max(1.0) as u16)
}

fn with_alpha(rgb: u32, a: f32) -> Rgba {
    rgba((rgb << 8) | (a.clamp(0.0, 1.0) * 255.0).round() as u32)
}

/// HUD lettering with CSS-style letter-spacing (GPUI has none): one box per glyph.
fn tracked(s: &str, spacing: f32) -> Div {
    div()
        .flex()
        .flex_none()
        .children(s.chars().map(|c| div().mr(px(spacing)).child(if c == ' ' { "\u{00A0}".to_string() } else { c.to_string() })))
}

struct Palette {
    frame: u32,
    accent: u32,
    surface: u32,
    text: u32,
    dim: u32,
    name: u32,
    slash: u32,
}

const HEARTS: [(f32, f32, f32, f32, u32); 4] = [
    // dx, dy, size, delay ms, colour
    (-10.0, -46.0, 24.0, 0.0, 0xff3e83),
    (29.0, -36.0, 17.0, 65.0, 0xffa7dc),
    (-34.0, -25.0, 12.0, 135.0, 0xff1f63),
    (15.0, -63.0, 9.0, 180.0, 0xffe0f0),
];

impl Render for Avatar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let l = self.layout;
        let sf = l.sf;
        if std::env::var_os("SA_DEBUG").is_some() {
            eprintln!(
                "render: frame {} loaded {} shown {:?} at {} ms",
                self.anim.frame(),
                self.loaded,
                self.bubble.as_ref().map(|b| b.shown),
                self.t_start.elapsed().as_millis()
            );
        }
        let composited = self.x11.composited();
        let pal = Palette {
            frame: self.pack.tone_or("frame", 0x8ef5ed),
            accent: self.pack.tone_or("accent", 0xc6fcf0),
            surface: self.pack.tone_or("surface", 0x0c131a),
            text: self.pack.tone_or("text", 0xe5edf0),
            dim: self.pack.tone_or("dim", 0x93adba),
            name: self.pack.tone_or("name", 0xf4ed75),
            slash: self.pack.tone_or("slash", 0xff3c68),
        };
        let frame = self.frame_or_default(self.anim.frame());
        let image = frame.map(|f| f.image.clone());
        let rects = frame.map(|f| f.rects.clone()).unwrap_or_default();
        let has_frame = frame.is_some();
        // The tag rides just above the current frame's head (sitting poses are lower).
        let tag_bottom = frame.map_or(l.tag_bottom, |f| (l.avatar_y + f.top - (TAG_GAP * sf).round() as i32).max(l.tag_bottom));
        let entity = cx.entity();
        // Once every frame is decoded, upload them all to the sprite atlas in one go so the
        // first switch to a new frame never stalls on an upload.
        let warm: Vec<Arc<RenderImage>> = if self.loaded && !self.warmed {
            self.warmed = true;
            self.frames.iter().flatten().map(|f| f.image.clone()).collect()
        } else {
            Vec::new()
        };

        let bubble_cell: Cell2 = Rc::new(Cell::new(None));
        let menu_cell: Cell2 = Rc::new(Cell::new(None));
        let tag_cell: Cell2 = Rc::new(Cell::new(None));

        let mut root = div().size_full().relative().font_family(self.font.clone()).text_color(rgb(pal.text));

        // The body.
        root = root.child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    if !warm.is_empty() {
                        let off = ContentMask { bounds: Bounds::new(point(px(-8.0), px(-8.0)), size(px(1.0), px(1.0))) };
                        window.with_content_mask(Some(off), |window| {
                            for img in warm {
                                window.paint_image(bounds, bounds, Corners::default(), img, 0, false).ok();
                            }
                        });
                    }
                    if let Some(img) = image {
                        window.paint_image(bounds, bounds, Corners::default(), img, 0, false).ok();
                    }
                    let e = entity.clone();
                    window.on_mouse_event(move |ev: &MouseDownEvent, phase, _, cx| {
                        if phase == DispatchPhase::Bubble {
                            e.update(cx, |v, cx| v.mouse_down(ev, cx));
                        }
                    });
                    let e = entity.clone();
                    window.on_mouse_event(move |ev: &MouseMoveEvent, phase, _, cx| {
                        if phase == DispatchPhase::Bubble {
                            e.update(cx, |v, cx| v.mouse_move(ev, cx));
                        }
                    });
                    let e = entity.clone();
                    window.on_mouse_event(move |ev: &MouseUpEvent, phase, _, cx| {
                        if phase == DispatchPhase::Bubble {
                            e.update(cx, |v, cx| v.mouse_up(ev, cx));
                        }
                    });
                },
            )
            .absolute()
            .left(l.lp(l.avatar_x))
            .top(l.lp(l.avatar_y))
            .w(l.lp(l.geo.width as i32))
            .h(l.lp(l.geo.height as i32)),
        );

        // Status tag, just above the head.
        if self.frames.iter().any(Option::is_some) {
            let (label, tone) = self.status();
            let tag = div()
                .flex()
                .items_center()
                .px(px(9.0))
                .py(px(4.0))
                .border_1()
                .border_color(rgb(tone))
                .bg(with_alpha(pal.surface, 0.97))
                .text_color(rgb(tone))
                .text_size(px(TAG_FONT))
                .line_height(px(TAG_FONT + 1.0))
                .font_weight(FontWeight::SEMIBOLD)
                .child(tracked(&label, 1.6));
            root = root.child(
                div()
                    .absolute()
                    .left_0()
                    .w(l.lp(l.win_w))
                    .bottom(l.lp(l.win_h - tag_bottom))
                    .flex()
                    .justify_center()
                    // Centre on the face, not the window.
                    .pl(l.lp(2 * l.face_x - l.win_w).max(px(0.0)))
                    .pr(l.lp(l.win_w - 2 * l.face_x).max(px(0.0)))
                    .child(probed(tag, &tag_cell)),
            );
        }

        // Speech bubble (or the focus-probe input).
        let fade = self.bubble.as_ref().map_or(1.0, |b| if composited { b.fade.max(0.0) } else { 1.0 });
        let bubble = if let Some(text) = &self.focus_probe {
            let focused = self.focus.is_focused(window);
            let body = if text.is_empty() { "click here, then type".to_string() } else { text.clone() };
            Some((if focused { "FOCUS PROBE // KEYS OK" } else { "FOCUS PROBE // CLICK ME" }.to_string(), body, true))
        } else {
            self.bubble.as_ref().map(|b| ("INCOMING TRANSMISSION".to_string(), b.text[..b.shown].iter().collect(), false))
        };
        let show_tail = bubble.is_some();
        if let Some((header, body, probe)) = bubble {
            let (vis_l, vis_r) = self.vis;
            let max_w = ((vis_r - vis_l) as f32 / sf - 2.0 * MARGIN).min(BUBBLE_MAX_W);
            let (frame_c, accent_c, bar_c, surface_c) = (pal.frame, pal.accent, pal.slash, pal.surface);
            let panel_bg = canvas(
                |_, _, _| (),
                move |b, _, window, _| {
                    let (x, y) = (f32::from(b.origin.x), f32::from(b.origin.y));
                    let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
                    let poly = |inset: f32| {
                        let c = CUT.min(w / 4.0).min(h / 3.0);
                        let d = c + inset * (std::f32::consts::SQRT_2 - 1.0);
                        [
                            (x + d, y + inset),
                            (x + w - inset, y + inset),
                            (x + w - inset, y + h - d),
                            (x + w - d, y + h - inset),
                            (x + inset, y + h - inset),
                            (x + inset, y + d),
                        ]
                    };
                    let pts = |v: &[(f32, f32)]| v.iter().map(|&(x, y)| point(px(x), px(y))).collect::<Vec<_>>();
                    let mut surface = PathBuilder::fill();
                    surface.add_polygon(&pts(&poly(0.0)), true);
                    if let Ok(p) = surface.build() {
                        window.paint_path(p, with_alpha(surface_c, 0.94 * fade));
                    }
                    let inset = OUTLINE / 2.0 + 0.35;
                    let mut edge = PathBuilder::stroke(px(OUTLINE));
                    edge.add_polygon(&pts(&poly(inset)), true);
                    if let Ok(p) = edge.build() {
                        window.paint_path(p, with_alpha(frame_c, 0.74 * fade));
                    }
                    // The bright tab: a 40 px run of the outline, 27 px along the top edge.
                    let x0 = poly(inset)[0].0 + 27.0;
                    let mut tab = PathBuilder::stroke(px(OUTLINE));
                    tab.move_to(point(px(x0), px(y + inset)));
                    tab.line_to(point(px((x0 + 40.0).min(x + w - inset)), px(y + inset)));
                    if let Ok(p) = tab.build() {
                        window.paint_path(p, with_alpha(accent_c, 0.9 * fade));
                    }
                    // Signal bar down the left side.
                    if h > 34.0 {
                        window.paint_quad(fill(
                            Bounds::new(point(px(x + 7.0), px(y + 16.0)), size(px(2.0), px(h - 28.0))),
                            with_alpha(bar_c, fade),
                        ));
                    }
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full();
            let mut panel = div()
                .id("bubble")
                .relative()
                .max_w(px(max_w))
                .min_w(px(200.0))
                .flex()
                .flex_col()
                .gap(px(5.0))
                .pt(px(12.0))
                .pr(px(15.0))
                .pb(px(14.0))
                .pl(px(19.0))
                .child(panel_bg)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(9.0))
                        .line_height(px(12.0))
                        .child(
                            div()
                                .flex()
                                .text_size(px(10.0))
                                .font_weight(FontWeight::BOLD)
                                .child(tracked(&self.name, 1.8).text_color(with_alpha(pal.name, fade)))
                                .child(div().ml(px(2.0)).text_color(with_alpha(pal.slash, fade)).child("/")),
                        )
                        .child(
                            tracked(&header, 1.5)
                                .text_size(px(9.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(with_alpha(pal.dim, fade)),
                        ),
                )
                .child(
                    // Long text keeps its tail in view: bottom-aligned, clipped at the top.
                    div().max_h(px(BODY_MAX_H)).overflow_hidden().flex().flex_col().justify_end().child(
                        div()
                            .flex_none()
                            .text_size(px(BODY_FONT))
                            .line_height(px(BODY_LINE))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(with_alpha(pal.text, fade))
                            .child(body),
                    ),
                );
            if probe {
                panel = panel
                    .track_focus(&self.focus)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            let r = this.x11.take_focus();
                            window.focus(&this.focus, cx);
                            let fw = this.x11.focused_window().unwrap_or(0);
                            eprintln!(
                                "focus-probe: SetInputFocus err={:?}; X focus now 0x{fw:x} (ours 0x{:x})",
                                r.err(),
                                this.x11.win
                            );
                            cx.notify();
                        }),
                    )
                    .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                        eprintln!("focus-probe: key down `{}` char {:?}", ev.keystroke.key, ev.keystroke.key_char);
                        if let Some(t) = &mut this.focus_probe {
                            match ev.keystroke.key.as_str() {
                                "backspace" => {
                                    t.pop();
                                }
                                "enter" => t.push('\n'),
                                _ => {
                                    if let Some(c) = &ev.keystroke.key_char {
                                        t.push_str(c);
                                    }
                                }
                            }
                        }
                        cx.notify();
                    }));
            }
            root = root.child(
                div()
                    .absolute()
                    .left(l.lp(vis_l) + px(MARGIN))
                    .right(l.lp(l.win_w - vis_r) + px(MARGIN))
                    .bottom(l.lp(l.win_h - l.bubble_bottom))
                    .flex()
                    .justify_center()
                    .items_end()
                    .child(probed(panel, &bubble_cell)),
            );
        }

        // Context menu.
        if let Some((at, _)) = self.menu {
            let frame_c = pal.frame;
            let item = |label: &'static str| {
                div()
                    .px(px(10.0))
                    .py(px(5.0))
                    .text_size(px(11.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(frame_c))
                    .hover(|s| s.bg(with_alpha(frame_c, 0.2)))
                    .child(tracked(label, 1.2))
            };
            let max_x = l.lp(self.vis.1) - px(150.0);
            let menu = div()
                .w(px(140.0))
                .flex()
                .flex_col()
                .py(px(4.0))
                .bg(with_alpha(pal.surface, 0.97))
                .border_1()
                .border_color(with_alpha(pal.frame, 0.62))
                .child(item("RESET POSITION").id("reset").on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.reset_position(cx);
                    }),
                ))
                .child(item("HIDE").id("hide").on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.quit(cx);
                    }),
                ));
            root = root.child(
                div()
                    .absolute()
                    .left(if at.x > max_x { max_x } else { at.x })
                    .top(at.y)
                    .child(probed(menu, &menu_cell)),
            );
        }

        // Last: the bubble tail and headpat hearts (they need the laid-out bubble and the
        // click point), then the X input/bounding shape for exactly what this frame draws.
        let (ax, ay) = (l.avatar_x as i16, l.avatar_y as i16);
        let x11 = self.x11.clone();
        let last = self.last_shape.clone();
        let first = self.first_frame.clone();
        let t_start = self.t_start;
        let ground_px = (l.ground_x as i16, (l.ground_y - 1) as i16, 1u16, 1u16);
        let face_x = l.face_x as f32 / sf;
        let hearts = self.hearts.map(|(t0, at)| (Instant::now().duration_since(t0).as_millis() as f32, at));
        root.child(
            canvas(
                |_, _, _| (),
                move |_, _, window, _| {
                    let mut input: Vec<ShapeRect> = rects.iter().map(|r| (r.0 + ax, r.1 + ay, r.2, r.3)).collect();
                    let mut visible = input.clone();
                    let dev = |pts: &[(f32, f32)]| pts.iter().map(|&(x, y)| (x * sf, y * sf)).collect::<Vec<_>>();

                    if let Some(b) = bubble_cell.get() {
                        input.push(to_device(b, sf));
                        let (bx, by) = (f32::from(b.origin.x), f32::from(b.origin.y));
                        let (bw, bh) = (f32::from(b.size.width), f32::from(b.size.height));
                        let c = CUT.min(bw / 4.0).min(bh / 3.0);
                        let outline = [(bx + c, by), (bx + bw, by), (bx + bw, by + bh - c), (bx + bw - c, by + bh), (bx, by + bh), (bx, by + c)];
                        shape::poly_rows(&dev(&outline), &mut visible);
                        if show_tail {
                            // Capri's tail: a cyan wedge hanging off the bottom edge, its point
                            // leaning toward the head, set just left of the status tag.
                            let bottom = by + bh;
                            let anchor = tag_cell.get().map_or(face_x - 24.0, |t| f32::from(t.origin.x) - 4.0);
                            let right = anchor.min(bx + bw - c - 6.0).max(bx + 30.0);
                            let left = right - TAIL_W;
                            let tri = [(left, bottom - 1.0), (left + TAIL_W * 0.8, bottom - 1.0), (right, bottom + TAIL_H)];
                            let mut wedge = PathBuilder::fill();
                            wedge.add_polygon(&tri.map(|(x, y)| point(px(x), px(y))), true);
                            if let Ok(p) = wedge.build() {
                                window.paint_path(p, with_alpha(pal.frame, 0.8 * fade));
                            }
                            shape::poly_rows(&dev(&tri), &mut visible);
                        }
                    }
                    if let Some(b) = menu_cell.get() {
                        input.push(to_device(b, sf));
                        visible.push(to_device(b, sf));
                    }
                    if let Some(b) = tag_cell.get() {
                        visible.push(to_device(b, sf));
                    }
                    if let Some((t, at)) = hearts {
                        let (ox, oy) = (f32::from(at.x), f32::from(at.y).max(70.0));
                        for (dx, dy, size, delay, color) in HEARTS {
                            let p = ((t - delay) / 1040.0).clamp(0.0, 1.0);
                            if p <= 0.0 || p >= 1.0 {
                                continue;
                            }
                            let ease = 1.0 - (1.0 - p).powi(3);
                            let alpha = if p < 0.18 { p / 0.18 } else if p < 0.58 { 1.0 } else { 1.0 - (p - 0.58) / 0.42 };
                            let scale = if p < 0.18 { 0.22 + 0.9 * p / 0.18 } else { 1.12 - 0.4 * (p - 0.18) / 0.82 };
                            let pts = shape::heart(ox + dx * ease, oy + dy * ease, size * scale);
                            let mut pb = PathBuilder::fill();
                            pb.add_polygon(&pts.iter().map(|&(x, y)| point(px(x), px(y))).collect::<Vec<_>>(), true);
                            if let Ok(path) = pb.build() {
                                // Without a compositor alpha can't fade; hearts pop instead.
                                let a = if composited { alpha } else { 1.0 };
                                window.paint_path(path, with_alpha(color, a));
                            }
                            if !composited && alpha > 0.35 {
                                shape::poly_rows(&dev(&pts), &mut visible);
                            }
                        }
                    }
                    // Never an empty bounding shape: X then reports the window fully obscured
                    // and GPUI stops drawing it for good.
                    if visible.is_empty() {
                        visible.push(ground_px);
                    }
                    let mut last = last.borrow_mut();
                    if last.0 != input || last.1 != visible {
                        x11.set_shape(&input, &visible).ok();
                        *last = (input, visible);
                    }
                    if has_frame && !first.get() {
                        first.set(true);
                        eprintln!("first-frame: painted {} ms after process start", t_start.elapsed().as_millis());
                    }
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
    }
}
