//! The avatar view: paints the current frame, the HUD (status tag, speech bubble, menu,
//! headpat hearts), handles pointer input, and keeps the X input/bounding shape in step
//! with what is drawn. All state changes go through `apply` (commands) or pointer handlers.

use crate::anim::{Animator, Mood, Rng, Seq};
use crate::control::Command;
use crate::frames::{Frame, Geometry};
use crate::pack::{MoodSpec, Pack};
use crate::platform::x11::{ShapeRect, X11};
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
    pub bubble_bottom: i32,
    pub tag_y: i32,
    /// Ground point (the feet) inside the window.
    pub ground_x: i32,
    pub ground_y: i32,
}

const MARGIN: f32 = 8.0;
const BUBBLE_AREA: f32 = 196.0;
const TAG_ROW: f32 = 24.0;
const MIN_WIDTH: f32 = 360.0;

impl Layout {
    pub fn new(pack: &Pack, sf: f32, body_height: f32) -> Layout {
        let geo = Geometry::new(pack, body_height * sf);
        let d = |l: f32| (l * sf).round() as i32;
        let win_w = (geo.width as i32).max(d(MIN_WIDTH));
        let bubble_bottom = d(BUBBLE_AREA);
        let tag_y = bubble_bottom + d(4.0);
        let avatar_y = tag_y + d(TAG_ROW);
        let avatar_x = (win_w - geo.width as i32) / 2;
        let c = pack.catalog.canonical_anchor;
        let (gx, gy) = geo.to_device(c.x, c.y);
        Layout {
            sf,
            geo,
            win_w,
            win_h: avatar_y + geo.height as i32,
            avatar_x,
            avatar_y,
            bubble_bottom,
            tag_y,
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
    timer: Option<Task<()>>,
    last_shape: Rc<RefCell<(Vec<ShapeRect>, Vec<ShapeRect>)>>,
    t_start: Instant,
    first_frame: Rc<Cell<bool>>,
    focus: FocusHandle,
    focus_probe: Option<String>,
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
            timer: None,
            last_shape: Rc::new(RefCell::new((vec![(0, 0, 1, 1)], vec![]))),
            t_start: init.t_start,
            first_frame: Rc::new(Cell::new(false)),
            focus: cx.focus_handle(),
            focus_probe: init.focus_probe.then(String::new),
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

    fn schedule(&mut self, cx: &mut Context<Self>) {
        let Some(at) = self.next_deadline() else {
            self.timer = None;
            return;
        };
        let delay = at.saturating_duration_since(Instant::now());
        self.timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update(cx, |v, cx| v.tick(cx)).ok();
        }));
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
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
                b.fade -= 1.0 / FADE_STEPS as f32;
                b.hide_at = Some(now + FADE_TICK);
            }
            if b.fade <= 0.01 {
                self.bubble = None;
            }
        }
        if typed_out && self.mood == Mood::Talking {
            self.set_mood(Mood::Idle, now);
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
        serde_json::json!({
            "mood": self.mood.key(),
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

    fn move_to(&mut self, o: (i32, i32), persist: bool) {
        let o = Self::clamp(&self.x11, &self.layout, o);
        if o != self.origin {
            self.origin = o;
            self.x11.move_to(o.0, o.1).ok();
        }
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

/// A zero-cost child that reports its parent's laid-out bounds.
fn bounds_probe(cell: Rc<Cell<Option<Bounds<Pixels>>>>) -> impl IntoElement {
    canvas(move |b, _, _| cell.set(Some(b)), |_, _, _, _| {}).absolute().top_0().left_0().size_full()
}

fn to_device(b: Bounds<Pixels>, sf: f32) -> ShapeRect {
    let x0 = (f32::from(b.origin.x) * sf).floor();
    let y0 = (f32::from(b.origin.y) * sf).floor();
    let x1 = (f32::from(b.origin.x + b.size.width) * sf).ceil();
    let y1 = (f32::from(b.origin.y + b.size.height) * sf).ceil();
    (x0 as i16, y0 as i16, (x1 - x0).max(1.0) as u16, (y1 - y0).max(1.0) as u16)
}

const CYAN: u32 = 0x8ef5ed;
const PANEL: u32 = 0x0a1318;
const TEXT: u32 = 0xe5edf0;
const DIM: u32 = 0x7d8d96;
const NAME: u32 = 0xf4ed75;
const SLASH: u32 = 0xff5679;

fn with_alpha(rgb: u32, a: u8) -> Rgba {
    rgba((rgb << 8) | a as u32)
}

impl Render for Avatar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let l = self.layout;
        if std::env::var_os("SA_DEBUG").is_some() {
            eprintln!("render: frame {} loaded {} at {} ms", self.anim.frame(), self.loaded, self.t_start.elapsed().as_millis());
        }
        let frame = self.frame_or_default(self.anim.frame());
        let image = frame.map(|f| f.image.clone());
        let rects = frame.map(|f| f.rects.clone()).unwrap_or_default();
        let entity = cx.entity();

        let bubble_cell: Rc<Cell<Option<Bounds<Pixels>>>> = Rc::new(Cell::new(None));
        let menu_cell: Rc<Cell<Option<Bounds<Pixels>>>> = Rc::new(Cell::new(None));
        let tag_cell: Rc<Cell<Option<Bounds<Pixels>>>> = Rc::new(Cell::new(None));
        let hearts_cell: Rc<Cell<Option<Bounds<Pixels>>>> = Rc::new(Cell::new(None));

        let mut root = div().size_full().relative().font_family(self.font.clone()).text_color(rgb(TEXT));

        // The body.
        root = root.child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
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

        // Status tag: NAME // STATUS, with a HUD rail running off to the right.
        if self.loaded || self.frames.iter().any(Option::is_some) {
            let (label, tone) = self.status();
            root = root.child(
                div()
                    .absolute()
                    .left(px(MARGIN))
                    .right(px(MARGIN))
                    .top(l.lp(l.tag_y))
                    .h(px(TAG_ROW - 4.0))
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .relative()
                            .flex_none()
                            .px(px(8.0))
                            .py(px(3.0))
                            .border_1()
                            .border_color(rgb(tone))
                            .bg(with_alpha(0x091218, 0xf0))
                            .text_color(rgb(tone))
                            .text_size(px(10.0))
                            .line_height(px(12.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .whitespace_nowrap()
                            .child(format!("{} // {}", self.name, label))
                            .child(bounds_probe(tag_cell.clone())),
                    )
                    .child(div().flex_1().h(px(1.0)).bg(with_alpha(tone, 0x70)))
                    .child(div().flex_none().w(px(14.0)).h(px(3.0)).bg(rgb(tone))),
            );
        }

        // Speech bubble (or the focus-probe input).
        let bubble = if let Some(text) = &self.focus_probe {
            let focused = self.focus.is_focused(window);
            let body = if text.is_empty() { "click here, then type".to_string() } else { format!("{text}▏") };
            Some((format!("{}", if focused { "FOCUS PROBE // KEYBOARD OK" } else { "FOCUS PROBE" }), body, 1.0, true))
        } else {
            self.bubble.as_ref().map(|b| {
                let mut s: String = b.text[..b.shown].iter().collect();
                if b.next_char.is_some() {
                    s.push('▍');
                }
                ("INCOMING TRANSMISSION".to_string(), s, b.fade.max(0.0), false)
            })
        };
        if let Some((header, body, fade, probe)) = bubble {
            let mut panel = div()
                .id("bubble")
                .relative()
                .max_w(px(l.win_w as f32 / l.sf - 2.0 * MARGIN))
                .min_w(px(150.0))
                .flex()
                .flex_col()
                .gap(px(5.0))
                .px(px(13.0))
                .pt(px(9.0))
                .pb(px(11.0))
                .bg(with_alpha(PANEL, 0xf2))
                .border_1()
                .border_color(with_alpha(CYAN, 0x8a))
                .opacity(fade)
                .child(
                    div()
                        .flex()
                        .gap(px(5.0))
                        .text_size(px(10.0))
                        .line_height(px(12.0))
                        .font_weight(FontWeight::BOLD)
                        .whitespace_nowrap()
                        .child(div().text_color(rgb(NAME)).child(self.name.clone()))
                        .child(div().text_color(rgb(SLASH)).child("/"))
                        .child(div().text_color(rgb(DIM)).child(header)),
                )
                .child(
                    div()
                        .max_h(px(150.0))
                        .overflow_hidden()
                        .flex()
                        .flex_col()
                        .justify_end()
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(15.0))
                                .line_height(px(20.0))
                                .font_weight(FontWeight::MEDIUM)
                                .child(body),
                        ),
                )
                // Corner tabs: brighter segments on the frame, HUD-style.
                .child(div().absolute().top(px(-1.0)).left(px(14.0)).w(px(46.0)).h(px(2.0)).bg(rgb(CYAN)))
                .child(div().absolute().bottom(px(-1.0)).right(px(18.0)).w(px(22.0)).h(px(2.0)).bg(rgb(CYAN)))
                .child(div().absolute().top(px(-1.0)).right(px(-1.0)).w(px(9.0)).h(px(2.0)).bg(rgb(CYAN)))
                .child(div().absolute().top(px(-1.0)).right(px(-1.0)).w(px(2.0)).h(px(9.0)).bg(rgb(CYAN)))
                .child(bounds_probe(bubble_cell.clone()));
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
                                "focus-probe: SetInputFocus {:?}; X focus now 0x{fw:x} (ours 0x{:x})",
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
                    .left(px(MARGIN))
                    .right(px(MARGIN))
                    .bottom(l.lp(l.win_h - l.bubble_bottom))
                    .flex()
                    .justify_center()
                    .child(panel),
            );
        }

        // Headpat hearts.
        if let Some((t0, at)) = self.hearts {
            let t = Instant::now().duration_since(t0).as_millis() as f32;
            let specs: [(f32, f32, f32, f32, u32); 4] = [
                (-10.0, -46.0, 24.0, 0.0, 0xff3e83),
                (29.0, -36.0, 17.0, 65.0, 0xffa7dc),
                (-34.0, -25.0, 12.0, 135.0, 0xff1f63),
                (15.0, -63.0, 9.0, 180.0, 0xffe0f0),
            ];
            let mut hearts = div()
                .absolute()
                .left(at.x - px(60.0))
                .top(at.y - px(90.0))
                .w(px(120.0))
                .h(px(110.0))
                .child(bounds_probe(hearts_cell.clone()));
            for (x, y, size, delay, color) in specs {
                let p = ((t - delay) / 1040.0).clamp(0.0, 1.0);
                if p <= 0.0 {
                    continue;
                }
                let ease = 1.0 - (1.0 - p).powi(3);
                let alpha = if p < 0.18 { p / 0.18 } else if p < 0.58 { 1.0 } else { 1.0 - (p - 0.58) / 0.42 };
                let scale = if p < 0.18 { 0.22 + 0.9 * p / 0.18 } else { 1.12 - 0.4 * (p - 0.18) / 0.82 };
                let s = size * scale;
                hearts = hearts.child(
                    div()
                        .absolute()
                        .left(px(60.0 + x * ease - s / 2.0))
                        .top(px(90.0 + y * ease - s / 2.0))
                        .text_size(px(s))
                        .line_height(px(s))
                        .text_color(rgb(color))
                        .opacity(alpha.clamp(0.0, 1.0))
                        .child("♥"),
                );
            }
            root = root.child(hearts);
        }

        // Context menu.
        if let Some((at, _)) = self.menu {
            let item = |label: &'static str| {
                div()
                    .px(px(10.0))
                    .py(px(5.0))
                    .text_size(px(11.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(CYAN))
                    .hover(|s| s.bg(with_alpha(CYAN, 0x30)))
                    .child(label)
            };
            let max_x = px(l.win_w as f32 / l.sf - 150.0);
            root = root.child(
                div()
                    .absolute()
                    .left(if at.x > max_x { max_x } else { at.x })
                    .top(at.y)
                    .w(px(140.0))
                    .flex()
                    .flex_col()
                    .py(px(4.0))
                    .bg(with_alpha(PANEL, 0xf5))
                    .border_1()
                    .border_color(with_alpha(CYAN, 0x8a))
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
                    ))
                    .child(bounds_probe(menu_cell.clone())),
            );
        }

        // Last: push the input/bounding shape for exactly what this frame draws.
        let (ax, ay) = (l.avatar_x as i16, l.avatar_y as i16);
        let x11 = self.x11.clone();
        let last = self.last_shape.clone();
        let first = self.first_frame.clone();
        let t_start = self.t_start;
        let has_frame = frame.is_some();
        let ground_px = (l.ground_x as i16, (l.ground_y - 1) as i16, 1u16, 1u16);
        root.child(canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                let sf = window.scale_factor();
                let mut input: Vec<ShapeRect> = rects.iter().map(|r| (r.0 + ax, r.1 + ay, r.2, r.3)).collect();
                for c in [&bubble_cell, &menu_cell] {
                    if let Some(b) = c.get() {
                        input.push(to_device(b, sf));
                    }
                }
                let mut visible = input.clone();
                for c in [&tag_cell, &hearts_cell] {
                    if let Some(b) = c.get() {
                        visible.push(to_device(b, sf));
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
                    window.on_next_frame(move |_, _| {
                        eprintln!("first-frame: next frame callback {} ms after process start", t_start.elapsed().as_millis());
                    });
                }
            },
        ))
    }
}
