//! The avatar view: paints the current frame and the HUD (status tag, speech bubble,
//! menu, hearts), runs the effects, handles pointer input and placement (ground, perch,
//! teleport), and keeps the X input/bounding shape in step with what is drawn. State
//! changes go through `apply` (commands), `tick` (time) or the pointer handlers.

use crate::anim::{Animator, Mood, Rng, Seq};
use crate::control::Command;
use crate::frames::{Frame, Geometry, NoteMasks};
use crate::fx::{self, BodyPlan, Bolt, Glint, Glitch, GlitchKind};
use crate::pack::{MoodSpec, MusicSpec, Pack, PerchSpec, Steps};
use crate::platform::x11::{WatchEvent, WinRect, X11};
use crate::shape::{self, ShapeRect};
use crate::state;
use crate::ticker::Ticker;
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
    /// Ground point (the feet) inside the window.
    pub ground_x: i32,
    pub ground_y: i32,
    /// Seat point of the perch frames inside the window (sitting on an edge).
    pub seat_y: Option<i32>,
    /// How far the lowest frame pixel reaches below the ground point.
    pub foot_pad: i32,
}

// Logical px. The HUD follows Capri's CSS (cyberpunk.css, avatar-detach.css).
const MARGIN: f32 = 8.0;
const TOP_MARGIN: f32 = 6.0;
const BODY_FONT: f32 = 20.0;
const LINE_H: f32 = 27.0;
const LINES_PER_PAGE: usize = 4;
const PAD_T: f32 = 11.0;
const PAD_B: f32 = 8.0;
const PAD_L: f32 = 21.0;
const PAD_R: f32 = 15.0;
const HEADER_H: f32 = 12.0;
const HEADER_GAP: f32 = 8.0;
const FOOTER_GAP: f32 = 5.0;
const FOOTER_H: f32 = 21.0;
const PANEL_MIN_W: f32 = 340.0;
const PANEL_MAX_W: f32 = 470.0;
const PANEL_MAX_H: f32 = PAD_T + HEADER_H + HEADER_GAP + LINES_PER_PAGE as f32 * LINE_H + FOOTER_GAP + FOOTER_H + PAD_B;
const CUT: f32 = 12.0;
const TAIL_W: f32 = 26.0;
const TAIL_H: f32 = 16.0;
const TIP_GAP: f32 = 2.0;
const TAG_FONT: f32 = 13.0;
/// Clear air between the status tag and the hair right under it (logical px).
const TAG_GAP: f32 = 13.0;
const MIN_WIDTH: f32 = 500.0;
/// Everything that touches a perch edge keeps this far from the window's corners (logical px).
const EDGE_MARGIN: f32 = 60.0;
/// Where a perch sits by default, as a fraction of the window's width.
const PERCH_AT: f32 = 0.75;
/// Feet stay this far above the monitor's bottom edge (device px).
const GROUND_ROOM: i32 = 6;

impl Layout {
    pub fn new(pack: &Pack, sf: f32, body_height: f32) -> Layout {
        let geo = Geometry::new(pack, body_height * sf);
        let d = |l: f32| (l * sf).round() as i32;
        let win_w = (geo.width as i32).max(d(MIN_WIDTH));
        let avatar_x = (win_w - geo.width as i32) / 2;
        // Room above the highest frame for the bubble and its tail.
        let min_top = pack
            .frames
            .iter()
            .map(|f| geo.to_device(0, f.bounds.y + pack.offset(f).1).1)
            .fold(f32::MAX, f32::min);
        let avatar_y = d(TOP_MARGIN + PANEL_MAX_H + TAIL_H + TIP_GAP) - min_top.round() as i32;
        let c = pack.catalog.canonical_anchor;
        let (gx, gy) = geo.to_device(c.x, c.y);
        let seat_y = pack.behaviour.perch.as_ref().and_then(|p| pack.index.get(&p.base)).map(|&i| {
            let f = &pack.frames[i];
            let (dx, dy) = pack.offset(f);
            avatar_y + geo.to_device(f.anchor.x + dx, f.anchor.y + dy).1.round() as i32
        });
        // Standing frames only (seat-anchored ones hang over an edge, not the floor).
        let foot_pad = pack
            .frames
            .iter()
            .filter(|f| f.anchor.name == "ground")
            .map(|f| {
                let (_, dy) = pack.offset(f);
                (geo.to_device(0, f.bounds.y + f.bounds.height + dy).1 - gy).ceil() as i32
            })
            .max()
            .unwrap_or(0)
            .max(0);
        Layout {
            sf,
            geo,
            foot_pad,
            win_w,
            win_h: avatar_y + geo.height as i32,
            avatar_x,
            avatar_y,
            ground_x: avatar_x + gx.round() as i32,
            ground_y: avatar_y + gy.round() as i32,
            seat_y,
        }
    }

    fn lp(&self, d: i32) -> Pixels {
        px(d as f32 / self.sf)
    }
}

#[derive(Clone, Debug)]
struct Wrapped {
    /// Char ranges of the wrapped lines (a line keeps its trailing space).
    lines: Vec<(usize, usize)>,
    /// Widest line, logical px.
    width: f32,
}

struct Bubble {
    text: Vec<char>,
    shown: usize,
    per_tick: usize,
    next_char: Option<Instant>,
    hide_at: Option<Instant>,
    wrap: Option<Wrapped>,
    /// Page picked with ‹ ›; None follows the newest text.
    page: Option<usize>,
    collapsed: bool,
}

impl Bubble {
    /// (current page, page count). Before wrapping: (0, 1).
    fn pages(&self) -> (usize, usize) {
        let Some(w) = &self.wrap else { return (0, 1) };
        let total = w.lines.len().max(1).div_ceil(LINES_PER_PAGE);
        let cursor = w.lines.iter().rposition(|&(s, _)| s < self.shown.max(1)).unwrap_or(0);
        let follow = cursor / LINES_PER_PAGE;
        (self.page.unwrap_or(follow).min(total - 1), total)
    }

    /// The revealed text of the current page, one string per line.
    fn page_lines(&self) -> Vec<String> {
        let Some(w) = &self.wrap else { return Vec::new() };
        let (p, _) = self.pages();
        w.lines
            .iter()
            .skip(p * LINES_PER_PAGE)
            .take(LINES_PER_PAGE)
            .take_while(|&&(s, _)| s < self.shown)
            .map(|&(s, e)| self.text[s..e.min(self.shown)].iter().collect::<String>().trim_end().to_string())
            .collect()
    }
}

const TYPE_TICK: Duration = Duration::from_millis(35);
const HEARTS_MS: u64 = 1_150;
/// First effect layer of the hearts (the bolt uses 0 and 1): a rim and a fill per heart.
const HEART_LAYER: usize = 6;
/// Effect layers 0..BOLT_LAYERS are the teleport bolt.
const BOLT_LAYERS: usize = 6;
/// The contact shadow under a perch, stacked right below the avatar window.
const SHADOW_LAYER: usize = 14;
/// Music notes: an outline and a fill layer each, at most seven notes.
const NOTE_LAYER: usize = 15;
const NOTES_MAX: usize = 7;
/// Notes are drawn this much larger than Capri's canvas sizes (our figure is smaller).
const NOTE_SCALE: f32 = 1.15;
/// Notes fade toward this tint (no alpha without a compositor).
const NOTE_TINT: u32 = 0x3a4656;
/// Note animation rate (shaped X windows; no GPUI redraw).
const NOTE_TICK: Duration = Duration::from_millis(40);
/// Hair glints: a glow and a core layer each, at most four at once. Shaped X windows like
/// the hearts, so a glint costs no GPUI redraw.
const GLINT_LAYER: usize = NOTE_LAYER + 2 * NOTES_MAX;
const GLINTS_MAX: usize = 4;
const GLINT_TICK: Duration = Duration::from_millis(60);

/// Device px of the largest note sprite for this geometry.
pub fn note_px(geo: &Geometry) -> u32 {
    (120.0 * geo.scale * NOTE_SCALE).ceil().max(8.0) as u32
}

struct MusicOn {
    t0: Instant,
    /// Autonomy sessions end on their own; `music on` runs until `music off`.
    until: Option<Instant>,
}
const FRAME_FAST: Duration = Duration::from_millis(16);
const FRAME_FX: Duration = Duration::from_millis(33);

struct Press {
    root: (i32, i32),
    origin: (i32, i32),
    head: bool,
    dragging: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Placement {
    Ground,
    /// On `win`'s top edge; `dx` = seat (or feet) x minus the window's left edge.
    Perch { win: u32, sit: bool, dx: i32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Leg {
    Vanish,
    Bolt(Instant),
    Appear,
}

struct Teleport {
    /// Seeds the vanish glitch, the bolt and the landing glitch.
    seed: u64,
    leg: Leg,
    to: (i32, i32),
    place: Placement,
    bolt: Option<Bolt>,
    moved: bool,
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
    /// Click reaction in progress: the status tag steps aside until then.
    reaction_until: Option<Instant>,
    menu: Option<(Point<Pixels>, Instant)>,
    press: Option<Press>,
    streak: (u32, Instant),
    /// Window position without the bob/hop offset.
    origin: (i32, i32),
    /// Where the window really is (origin + offset).
    applied: (i32, i32),
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
    // Life and effects.
    bob_t0: Instant,
    hop: Option<Instant>,
    next_hop: Option<Instant>,
    glitch: Option<Glitch>,
    next_glitch: Option<Instant>,
    squash: Option<Instant>,
    glints: Vec<Glint>,
    next_glint: Option<Instant>,
    teleport: Option<Teleport>,
    /// Body not drawn (between dematerialise and materialise).
    hidden: bool,
    quitting: bool,
    placement: Placement,
    /// Ground point to fall back to when a perch ends.
    home: (i32, i32),
    /// What touches a perch edge, x relative to the seat (sitting) / feet (standing).
    contact: [Option<(i32, i32)>; 2],
    /// Contact shadow: rects for the frame it was built from, and where it is shown.
    shadow: Option<(usize, Vec<ShapeRect>)>,
    shadow_at: Option<(i32, i32)>,
    /// Last laid-out width of the status tag (logical px), to place the next one.
    tag_w: Rc<Cell<f32>>,
    /// Last laid-out box of the status tag (window logical px), to tear it in a glitch.
    tag_box: Cell2,
    music: Option<MusicOn>,
    /// The teleport line's look (pack default, `bolt` dev command).
    bolt_style: fx::LineStyle,
    /// Dev: every teleport uses this seed (vanish, bolt and landing).
    bolt_seed: Option<u64>,
    notes: Vec<NoteMasks>,
    notes_shown: usize,
    glints_shown: usize,
    /// Breath/sway/nod as last drawn.
    pose_last: fx::Pose,
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

/// A period with ±25 % jitter, never under 250 ms (a 0 in the pack must not spin).
fn jitter(rng: &mut Rng, ms: u64) -> Duration {
    Duration::from_millis(((ms as f32 * rng.range(0.75, 1.3)) as u64).max(250))
}

impl Avatar {
    pub fn new(init: Init, cx: &mut Context<Self>) -> Avatar {
        let pack = init.pack;
        let bolt_style = fx::LineStyle::parse(&pack.behaviour.teleport.line_style).unwrap_or(fx::LineStyle::Glow);
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
        let l = init.layout;
        let home = (init.origin.0 + l.ground_x, init.origin.1 + l.ground_y);
        let now = Instant::now();
        Avatar {
            frames: (0..pack.frames.len()).map(|_| None).collect(),
            anim: Animator::new(default),
            name: init.name.to_uppercase().into(),
            font: font.into(),
            pack,
            layout: l,
            x11: init.x11,
            loaded: false,
            mood: Mood::Idle,
            mood_then: None,
            next_activity: None,
            autonomy_started: false,
            rng: Rng::seeded(),
            bubble: None,
            hearts: None,
            reaction_until: None,
            menu: None,
            press: None,
            streak: (0, now),
            origin: init.origin,
            applied: init.origin,
            ticker,
            _tick_loop: tick_loop,
            last_shape: Rc::new(RefCell::new((vec![(0, 0, 1, 1)], vec![]))),
            t_start: init.t_start,
            first_frame: Rc::new(Cell::new(false)),
            focus: cx.focus_handle(),
            focus_probe: init.focus_probe.then(String::new),
            warmed: false,
            vis,
            bob_t0: now,
            hop: None,
            next_hop: None,
            glitch: None,
            next_glitch: None,
            squash: None,
            glints: Vec::new(),
            next_glint: None,
            teleport: None,
            hidden: false,
            quitting: false,
            placement: Placement::Ground,
            home,
            contact: [None, None],
            shadow: None,
            shadow_at: None,
            tag_w: Rc::new(Cell::new(150.0)),
            tag_box: Rc::new(Cell::new(None)),
            music: None,
            bolt_style,
            bolt_seed: None,
            notes: Vec::new(),
            notes_shown: 0,
            glints_shown: 0,
            pose_last: fx::Pose::default(),
        }
    }

    pub fn set_notes(&mut self, notes: Vec<NoteMasks>) {
        self.notes = notes;
    }

    // ---- music --------------------------------------------------------------------

    fn music_spec(&self) -> Option<&MusicSpec> {
        self.pack.behaviour.music.as_ref()
    }

    /// The music pose for the current placement, if music is on and the pack has it.
    fn music_frame(&self) -> Option<usize> {
        self.music.as_ref()?;
        let spec = self.music_spec()?;
        let idx = |id: &str| self.pack.index.get(id).copied();
        if self.sitting() {
            spec.window.as_deref().and_then(idx).filter(|&i| self.pack.frames[i].anchor.name == "seat")
        } else {
            idx(&spec.floor).filter(|&i| self.pack.frames[i].anchor.name == "ground")
        }
    }

    fn music_on(&mut self, now: Instant, until: Option<Instant>) -> anyhow::Result<()> {
        let Some(spec) = self.music_spec() else { anyhow::bail!("this pack has no music") };
        if !self.pack.index.contains_key(&spec.floor) {
            anyhow::bail!("music frame `{}` is missing from the pack", spec.floor);
        }
        if self.mood != Mood::Idle {
            self.set_mood(Mood::Idle, now);
        }
        let was = self.music.replace(MusicOn { t0: now, until });
        self.relayer(now);
        if was.is_none() && self.loaded && self.teleport.is_none() {
            self.glitch = Some(Glitch::new(GlitchKind::Burst, now, self.rng.next()));
        }
        Ok(())
    }

    fn music_off(&mut self, now: Instant) {
        if self.music.take().is_some() {
            self.relayer(now);
            self.update_notes(now);
            if self.loaded && self.teleport.is_none() {
                self.glitch = Some(Glitch::new(GlitchKind::Burst, now, self.rng.next()));
            }
        }
    }

    /// Move the note layers to `now`; unmap the ones not in use.
    fn update_notes(&mut self, now: Instant) {
        let mut used = 0;
        let show = self.loaded && !self.hidden && !self.quitting && self.teleport.is_none() && !self.notes.is_empty();
        if let (true, Some(m), Some(fi), Some(spec)) = (show, self.music.as_ref(), self.music_frame(), self.music_spec()) {
            let src = &self.pack.frames[fi];
            let emitters = spec.emitters.get(&src.id).map(Vec::as_slice).unwrap_or(&[]);
            let (ox, oy) = self.pack.offset(src);
            let l = self.layout;
            let beat = 60.0 / spec.bpm.clamp(40.0, 240.0);
            let parts = fx::music_notes(now.saturating_duration_since(m.t0).as_secs_f32(), beat, emitters);
            let base = (self.applied.0 + l.avatar_x, self.applied.1 + l.avatar_y);
            for n in parts.iter().take(NOTES_MAX) {
                // No alpha: fade by shrinking and stepping the colour toward a dim tint, then
                // pop out. Four steps, no dither.
                if n.alpha < 0.14 {
                    continue;
                }
                let k = if n.alpha > 0.8 {
                    1.0
                } else if n.alpha > 0.55 {
                    0.72
                } else if n.alpha > 0.32 {
                    0.48
                } else {
                    0.28
                };
                let sprite = &self.notes[n.sprite % self.notes.len()];
                let (dx, dy) = l.geo.to_device(n.x.round() as i32 + ox, n.y.round() as i32 + oy);
                let size = ((n.size * l.geo.scale * NOTE_SCALE * (0.55 + 0.45 * k)).round() as i32).clamp(4, sprite.size as i32);
                let o = (base.0 + dx.round() as i32 - size / 2, base.1 + dy.round() as i32 - size / 2);
                let ai = frames_angle(n.deg);
                let (outline, fill) = &sprite.variants[ai];
                let scale = sprite.size as f32 / size as f32;
                let runs = |mask: &crate::frames::Mask| {
                    let mut out = Vec::new();
                    for y in 0..size {
                        let mut x = 0;
                        let on = |x: i32| mask.get((x as f32 * scale) as i32, (y as f32 * scale) as i32);
                        while x < size {
                            if on(x) {
                                let s0 = x;
                                while x < size && on(x) {
                                    x += 1;
                                }
                                out.push((s0 as i16, y as i16, (x - s0) as u16, 1u16));
                            } else {
                                x += 1;
                            }
                        }
                    }
                    out
                };
                let (ro, rf) = (runs(outline), runs(fill));
                let (lo, lf) = (NOTE_LAYER + 2 * used, NOTE_LAYER + 2 * used + 1);
                self.x11.fx_set(lf, mix_rgb(NOTE_TINT, sprite.fill_rgb, k), o, (size, size), &rf).ok();
                self.x11.fx_set_below(lo, lf, mix_rgb(NOTE_TINT, sprite.outline_rgb, k), o, (size, size), &ro).ok();
                used += 1;
            }
        }
        for k in used..self.notes_shown {
            self.x11.fx_unmap(NOTE_LAYER + 2 * k);
            self.x11.fx_unmap(NOTE_LAYER + 2 * k + 1);
        }
        if used > 0 || self.notes_shown > 0 {
            self.x11.flush();
        }
        self.notes_shown = used;
    }

    /// Move the glint layers to `now`; unmap the ones not in use.
    fn update_glints(&mut self, now: Instant) {
        let life_ms = self.pack.behaviour.life.sparkle_ms;
        let show = self.loaded && !self.hidden && !self.quitting && self.glitch.is_none() && self.teleport.is_none() && self.squash.is_none();
        let l = self.layout;
        let base = ((self.applied.0 + l.avatar_x) as f32, (self.applied.1 + l.avatar_y) as f32);
        let mut used = 0;
        if show {
            for g in &self.glints {
                if used >= GLINTS_MAX {
                    break;
                }
                let Some(sc) = g.scale(now, life_ms).filter(|&v| v > 0.02) else { continue };
                let (cx, cy, r) = (base.0 + g.x, base.1 + g.y, g.size * sc);
                let place = |pts: Vec<(f32, f32)>| {
                    let x0 = pts.iter().map(|p| p.0).fold(f32::MAX, f32::min).floor() as i32;
                    let y0 = pts.iter().map(|p| p.1).fold(f32::MAX, f32::min).floor() as i32;
                    let x1 = pts.iter().map(|p| p.0).fold(f32::MIN, f32::max).ceil() as i32;
                    let y1 = pts.iter().map(|p| p.1).fold(f32::MIN, f32::max).ceil() as i32;
                    let local: Vec<(f32, f32)> = pts.iter().map(|&(x, y)| (x - x0 as f32, y - y0 as f32)).collect();
                    let mut rows = Vec::new();
                    shape::poly_rows(&local, &mut rows);
                    ((x0, y0), (x1 - x0, y1 - y0), rows)
                };
                let (core, glow) = (GLINT_LAYER + 2 * used + 1, GLINT_LAYER + 2 * used);
                let (o, sz, rows) = place(fx::star(cx, cy, r, g.spin + 0.04));
                self.x11.fx_set(core, 0xffffff, o, sz, &rows).ok();
                let (o, sz, rows) = place(fx::star(cx, cy, r * 1.3, g.spin));
                self.x11.fx_set_below(glow, core, 0xffc2e2, o, sz, &rows).ok();
                used += 1;
            }
        }
        for k in used..self.glints_shown {
            self.x11.fx_unmap(GLINT_LAYER + 2 * k);
            self.x11.fx_unmap(GLINT_LAYER + 2 * k + 1);
        }
        if used > 0 || self.glints_shown > 0 {
            self.x11.flush();
        }
        self.glints_shown = used;
    }

    // ---- idle/music body motion --------------------------------------------------------

    fn pose_active(&self) -> bool {
        self.loaded
            && !self.hidden
            && !self.quitting
            && self.teleport.is_none()
            && self.press.is_none()
            && self.squash.is_none()
            && self.glitch.is_none()
    }

    fn pose_at(&self, now: Instant) -> fx::Pose {
        if !self.pose_active() {
            return fx::Pose::default();
        }
        let sf = self.layout.sf;
        if let (Some(m), Some(spec), Some(_)) = (&self.music, self.music_spec(), self.music_frame()) {
            // Held on twos (12 fps) like Capri's music body.
            let hold = 1000.0 / 12.0;
            let t = (now.saturating_duration_since(m.t0).as_secs_f32() * 1000.0 / hold).floor() * hold;
            let beats = t / (60_000.0 / spec.bpm.clamp(40.0, 240.0));
            let p = beats.fract();
            // Head down on the beat and back up (4 px), the upper body tilting side to side
            // over two beats (4 px at the top of the hair).
            let nod = (4.0 * sf * (0.5 + 0.5 * (p * std::f32::consts::TAU).cos())).round() as i32;
            let sway = (4.0 * sf * (beats * std::f32::consts::PI).sin()).round() as i32;
            return fx::Pose { breath: 0, nod, sway };
        }
        let life = &self.pack.behaviour.life;
        let t = now.saturating_duration_since(self.bob_t0);
        let breath = fx::bob(t, life.bob_px, life.bob_period_ms);
        // In 2 px steps once it's wide enough: every step is a redraw.
        let sway = if life.sway_px > 0 && life.sway_period_ms > 0 {
            let step = if life.sway_px >= 4 { 2.0 } else { 1.0 };
            let v = life.sway_px as f32 * (t.as_secs_f32() * 1000.0 / life.sway_period_ms as f32 * std::f32::consts::TAU).sin();
            ((v / step).round() * step) as i32
        } else {
            0
        };
        fx::Pose { breath, nod: 0, sway }
    }

    fn pose_next_change(&self, now: Instant) -> Option<Instant> {
        let life = &self.pack.behaviour.life;
        if !self.pose_active() || (self.music_frame().is_none() && life.bob_px <= 0 && life.sway_px <= 0) {
            return None;
        }
        let cur = self.pose_at(now);
        let step = Duration::from_millis(20);
        (1..=100).map(|k| now + step * k).find(|&t| self.pose_at(t) != cur).or(Some(now + Duration::from_secs(2)))
    }

    /// The rows a pose acts on for frame `fi`.
    fn pose_rows(&self, fi: usize, f: &Frame) -> fx::PoseRows {
        let l = &self.layout;
        let seat = self.pack.frames[fi].anchor.name == "seat";
        let line = if seat { l.seat_y.unwrap_or(l.ground_y) } else { l.ground_y } - l.avatar_y;
        let span = (line - f.top).max(1) as f32;
        let waist = line - (span * if seat { 0.3 } else { 0.5 }) as i32;
        let neck = f.face.1 + (span * 0.04) as i32;
        let music = self.music.is_some() && self.music_frame() == Some(fi);
        fx::PoseRows {
            y0: -l.avatar_y,
            h: l.geo.height as i32,
            top: f.top,
            shear_end: if music { waist } else { f.face.0 },
            neck,
            waist,
        }
    }

    /// Frames arrive from the background decoder: the default first, then the rest.
    pub fn add_frames(&mut self, decoded: Vec<(usize, Frame)>, complete: bool, cx: &mut Context<Self>) {
        let first = !self.frames.iter().any(Option::is_some);
        for (i, f) in decoded {
            self.frames[i] = Some(f);
        }
        let now = Instant::now();
        if first {
            // Materialise with a glitch, like Capri coming online.
            self.glitch = Some(Glitch::new(GlitchKind::Appear, now, self.rng.next()));
        }
        if complete && !self.loaded {
            self.loaded = true;
            self.contact = [self.measure_contact(true), self.measure_contact(false)];
            self.set_mood(Mood::Idle, now);
            self.next_glint = Some(now + Duration::from_millis(700));
            self.bob_t0 = now;
        }
        cx.notify();
        self.schedule(cx);
    }

    fn spec(&self, m: Mood) -> Option<&MoodSpec> {
        self.pack.behaviour.moods.get(m.key())
    }

    fn perch_spec(&self) -> Option<&PerchSpec> {
        self.pack.behaviour.perch.as_ref()
    }

    fn sitting(&self) -> bool {
        matches!(self.placement, Placement::Perch { sit: true, .. })
    }

    fn frame_or_default(&self, i: usize) -> Option<&Frame> {
        self.frames.get(i).and_then(|f| f.as_ref()).or_else(|| {
            let d = self.pack.frame(&self.pack.catalog.default_sprite).ok()?;
            self.frames.get(d)?.as_ref()
        })
    }

    fn loop_seq(&self, l: Option<&Steps>) -> Option<Seq> {
        let seq = self.pack.flatten(&l?.as_steps());
        (!seq.is_empty()).then_some(seq)
    }

    fn autonomy_gap(&mut self) -> Duration {
        let a = &self.pack.catalog.autonomy;
        let life = &self.pack.behaviour.life;
        let base = if self.autonomy_started {
            life.activity_gap_ms.unwrap_or(a.activity_gap_ms)
        } else {
            life.first_activity_ms.unwrap_or(a.first_activity_ms)
        };
        Duration::from_millis(((base as f32 * self.rng.range(0.75, 1.35)) as u64).max(500))
    }

    /// Base frame and loop for a mood, given where the avatar is (standing or perched).
    fn layer(&self, m: Mood) -> (usize, Option<Seq>) {
        let default = self.pack.frame(&self.pack.catalog.default_sprite).unwrap_or(0);
        if let Some(i) = self.music_frame() {
            return (i, None);
        }
        if self.sitting()
            && let Some(p) = self.perch_spec()
        {
            let id = p.moods.get(m.key()).unwrap_or(&p.base);
            return (self.pack.index.get(id).copied().unwrap_or(default), None);
        }
        let spec = self.spec(m);
        let base = spec.and_then(|s| self.pack.index.get(&s.base).copied()).unwrap_or(default);
        (base, self.loop_seq(spec.and_then(|s| s.r#loop.as_ref())))
    }

    fn activities(&self) -> &[Vec<crate::pack::Step>] {
        match self.perch_spec() {
            Some(p) if self.sitting() => &p.activities,
            _ => &self.pack.behaviour.activities,
        }
    }

    pub fn set_mood(&mut self, m: Mood, now: Instant) {
        if m != Mood::Idle {
            self.music = None;
        }
        let prev = self.mood;
        let mut seq: Seq = Vec::new();
        if !self.sitting() {
            if m != prev
                && let Some(exit) = self.spec(prev).and_then(|s| s.exit.clone())
            {
                seq.extend(self.pack.flatten(&exit));
            }
            if let Some(entry) = self.spec(m).and_then(|s| s.entry.clone()) {
                seq.extend(self.pack.flatten(&entry));
            }
        }
        let (base, looping) = self.layer(m);
        self.anim.set_layer(base, looping, now);
        if !seq.is_empty() {
            self.anim.play(seq, now);
        } else if m != prev {
            self.anim.stop_oneshot(now);
        }
        // A change of signal colour glitches, like Capri's state bursts.
        // Speaking comes and goes with every reply; that alone is not worth a burst.
        let chatter = m == Mood::Talking || prev == Mood::Talking;
        if m != prev && !chatter && self.loaded && self.teleport.is_none() && self.status_of(m).tone != self.status_of(prev).tone {
            let big = self.spec(m).is_some_and(|s| s.big) || self.spec(prev).is_some_and(|s| s.big);
            self.glitch = Some(Glitch::new(if big { GlitchKind::Big } else { GlitchKind::Burst }, now, self.rng.next()));
        }
        self.mood = m;
        let spec = self.spec(m).cloned();
        self.mood_then = spec
            .as_ref()
            .and_then(|s| s.then.as_ref())
            .and_then(|t| Some((now + Duration::from_millis(t.after_ms), Mood::parse(&t.mood)?)));
        let autonomous = spec.as_ref().map_or(m == Mood::Idle, |s| s.autonomy);
        self.next_activity = if autonomous && self.pack.catalog.autonomy.enabled && !self.activities().is_empty() {
            Some(now + self.autonomy_gap())
        } else {
            None
        };
        self.next_hop = spec.as_ref().and_then(|s| s.hop_every_ms).filter(|&p| p > 0).map(|p| now + jitter(&mut self.rng, p) / 2);
        self.next_glitch = spec.as_ref().and_then(|s| s.glitch_every_ms).filter(|&p| p > 0).map(|p| now + jitter(&mut self.rng, p));
    }

    /// Re-apply the current mood's layer after the placement changed (no entry/exit).
    fn relayer(&mut self, now: Instant) {
        let (base, looping) = self.layer(self.mood);
        self.anim.set_layer(base, looping, now);
        self.anim.stop_oneshot(now);
        let autonomous =
            self.music.is_none() && self.spec(self.mood).map_or(self.mood == Mood::Idle, |s| s.autonomy) && !self.activities().is_empty();
        let next = if autonomous { Some(now + self.autonomy_gap()) } else { None };
        self.next_activity = next;
    }

    fn status_of(&self, m: Mood) -> Status {
        let spec = self.spec(m);
        Status {
            label: spec.and_then(|s| s.status.clone()).unwrap_or_else(|| m.default_status().into()),
            tone: self.pack.tone(spec.and_then(|s| s.tone.as_deref())),
            invert: spec.is_some_and(|s| s.invert),
            glyph: spec.and_then(|s| s.glyph.clone()).unwrap_or_else(|| "dot".into()),
        }
    }

    fn status(&self) -> Status {
        if let (Some(_), Some(spec)) = (&self.music, self.music_spec()) {
            return Status {
                label: spec.status.clone().unwrap_or_else(|| "LISTENING".into()),
                tone: self.pack.tone(spec.tone.as_deref()),
                invert: false,
                glyph: spec.glyph.clone().unwrap_or_else(|| "note".into()),
            };
        }
        self.status_of(self.mood)
    }

    // ---- perch shadow ---------------------------------------------------------------

    /// Contact sliver for a perch: a solid dark band a few px thick on the window face right
    /// under the edge, as wide as the seat and thighs that rest on it (the run of the
    /// figure at the seat line that holds the seat point), minus the legs hanging in front.
    /// 1-bit friendly: no dither. Avatar-local device px.
    fn shadow_rects(&self, frame: usize) -> Vec<ShapeRect> {
        let l = &self.layout;
        let Some(p) = self.perch_spec() else { return Vec::new() };
        let (Some(f), Some(seat)) = (self.frames.get(frame).and_then(|f| f.as_ref()), l.seat_y) else { return Vec::new() };
        let thick = (p.sliver_px * l.sf).round() as i32;
        if thick <= 0 {
            return Vec::new();
        }
        let line = seat - l.avatar_y;
        let (w, h) = (f.mask.w as i32, f.mask.h as i32);
        let sx = l.ground_x - l.avatar_x;
        // The seat run: on a row just above the edge, the opaque span through the seat x.
        let row = line - (2.0 * l.sf) as i32;
        if !(0..h).contains(&row) || !f.mask.get(sx, row) {
            return Vec::new();
        }
        let (mut x0, mut x1) = (sx, sx);
        while x0 > 0 && f.mask.get(x0 - 1, row) {
            x0 -= 1;
        }
        while x1 < w - 1 && f.mask.get(x1 + 1, row) {
            x1 += 1;
        }
        let mut out = Vec::new();
        for y in (line + 1)..(line + 1 + thick).min(h) {
            let mut x = x0;
            while x <= x1 {
                if f.mask.get(x, y) {
                    x += 1;
                    continue;
                }
                let s0 = x;
                while x <= x1 && !f.mask.get(x, y) {
                    x += 1;
                }
                out.push((s0 as i16, y as i16, (x - s0) as u16, 1));
            }
        }
        out
    }

    /// Show, move or hide the contact shadow to match the placement.
    fn sync_shadow(&mut self) {
        let want = self.sitting() && self.loaded && !self.hidden && self.teleport.is_none() && !self.quitting && self.press.is_none();
        // The resting frame of the current layer (music swaps the pose), else the perch base.
        let base = Some(self.anim.base())
            .filter(|&i| self.pack.frames.get(i).is_some_and(|f| f.anchor.name == "seat"))
            .or_else(|| self.perch_spec().and_then(|p| self.pack.index.get(&p.base).copied()));
        let (Some(base), true) = (base, want) else {
            if self.shadow_at.take().is_some() {
                self.x11.fx_unmap(SHADOW_LAYER);
                self.x11.flush();
            }
            return;
        };
        if self.shadow.as_ref().map(|s| s.0) != Some(base) {
            self.shadow = Some((base, self.shadow_rects(base)));
            if self.shadow_at.take().is_some() {
                self.x11.fx_unmap(SHADOW_LAYER);
            }
        }
        let l = self.layout;
        let at = (self.applied.0 + l.avatar_x, self.applied.1 + l.avatar_y);
        if self.shadow_at != Some(at) {
            let rects = self.shadow.as_ref().map(|s| s.1.clone()).unwrap_or_default();
            let reshape = self.shadow_at.is_none();
            // Cut off at the window's bottom edge: the shadow falls on the window, not past it.
            let win = match self.placement {
                Placement::Perch { win, .. } => self.x11.toplevel_rect(win),
                Placement::Ground => None,
            };
            let h = win.map_or(l.geo.height as i32, |r| (r.y + r.h - at.1).clamp(1, l.geo.height as i32));
            let rgb = self.perch_spec().and_then(|p| u32::from_str_radix(p.sliver.trim_start_matches('#'), 16).ok()).unwrap_or(0x05060a);
            self.x11.fx_under(SHADOW_LAYER, rgb, at, (l.geo.width as i32, h), reshape.then_some(&rects[..])).ok();
            self.shadow_at = Some(at);
        }
    }

    /// Unmap every effect layer (the shadow too).
    fn fx_hide_all(&mut self) {
        self.x11.fx_hide();
        self.shadow_at = None;
    }

    fn fx_hide_bolt(&self) {
        for i in 0..BOLT_LAYERS {
            self.x11.fx_unmap(i);
        }
        self.x11.flush();
    }

    // ---- timing -------------------------------------------------------------------

    fn hop_allowed(&self) -> bool {
        self.loaded
            && !matches!(self.placement, Placement::Perch { sit: true, .. })
            && self.press.is_none()
            && self.teleport.is_none()
            && self.music.is_none()
    }

    fn next_deadline(&self) -> Option<Instant> {
        let now = Instant::now();
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
            take(Some((now + FRAME_FX).min(t0 + Duration::from_millis(HEARTS_MS))));
        }
        take(self.reaction_until);
        take(self.menu.map(|m| m.1));
        if self.glitch.is_some() {
            take(Some(now + FRAME_FX));
        }
        if self.teleport.as_ref().is_some_and(|t| matches!(t.leg, Leg::Bolt(_))) {
            take(Some(now + FRAME_FAST));
        }
        take(self.next_glitch);
        if self.squash.is_some() {
            take(Some(now + FRAME_FAST));
        }
        if !self.glints.is_empty() {
            take(Some(now + GLINT_TICK));
        }
        take(self.next_glint);
        take(self.pose_next_change(now));
        if self.music.is_some() {
            take(Some(now + NOTE_TICK));
            take(self.music.as_ref().and_then(|m| m.until));
        }
        if self.hop.is_some() {
            take(Some(now + FRAME_FAST));
        } else if self.hop_allowed() {
            take(self.next_hop);
        }
        t
    }

    fn schedule(&mut self, _cx: &mut Context<Self>) {
        self.sync_shadow();
        self.ticker.set(self.next_deadline());
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        let mut dirty = self.anim.tick(now);
        if let Some((at, m)) = self.mood_then
            && at <= now
        {
            self.set_mood(m, now);
            dirty = true;
        }
        if let Some(at) = self.next_activity
            && at <= now
            && !self.anim.busy()
            && !self.activities().is_empty()
        {
            let music = self
                .music_spec()
                .filter(|m| m.activity_chance > 0.0 && self.mood == Mood::Idle && self.music.is_none())
                .map(|m| (m.activity_chance, m.activity_ms));
            if let Some((chance, ms)) = music
                && self.rng.range(0.0, 1.0) < chance
                && self.music_on(now, Some(now + Duration::from_millis(ms))).is_ok()
            {
                self.autonomy_started = true;
            } else {
                let n = self.activities().len();
                let i = self.rng.pick(n);
                let seq = self.pack.flatten(&self.activities()[i]);
                let len: u64 = seq.iter().map(|s| s.1 as u64).sum();
                self.anim.play(seq, now);
                self.autonomy_started = true;
                self.next_activity = Some(now + Duration::from_millis(len) + self.autonomy_gap());
            }
            dirty = true;
        }
        let mut typed_out = false;
        if let Some(b) = &mut self.bubble {
            if let Some(at) = b.next_char
                && at <= now
            {
                b.shown = (b.shown + b.per_tick).min(b.text.len());
                dirty = true;
                if b.shown == b.text.len() {
                    b.next_char = None;
                    typed_out = true;
                    let hold = (3_000 + 45 * b.text.len().min(240) as u64).min(12_000);
                    b.hide_at = Some(now + Duration::from_millis(hold));
                } else {
                    b.next_char = Some(now + TYPE_TICK);
                }
            }
            if let Some(at) = b.hide_at
                && at <= now
            {
                self.bubble = None;
                dirty = true;
            }
        }
        // Keep SPEAKING up a beat after the last word, then settle.
        if typed_out && self.mood == Mood::Talking {
            self.mood_then = Some((now + Duration::from_millis(1_400), Mood::Idle));
        }
        if let Some((t0, _)) = self.hearts {
            dirty = true;
            if now.duration_since(t0) >= Duration::from_millis(HEARTS_MS) {
                self.hearts = None;
            }
        }
        if let Some(t) = self.reaction_until
            && t <= now
        {
            self.reaction_until = None;
            dirty = true;
        }
        if let Some((_, until)) = self.menu
            && until <= now
        {
            self.menu = None;
            dirty = true;
        }

        // Effects.
        if let Some(g) = self.glitch {
            dirty = true;
            if g.done(now) {
                self.glitch = None;
                self.glitch_done(now, cx);
            }
        }
        if let Some(at) = self.next_glitch
            && at <= now
        {
            if self.glitch.is_none() && self.teleport.is_none() && self.loaded {
                self.glitch = Some(Glitch::new(GlitchKind::Burst, now, self.rng.next()));
                dirty = true;
            }
            let p = self.spec(self.mood).and_then(|s| s.glitch_every_ms).filter(|&p| p > 0);
            self.next_glitch = p.map(|p| now + jitter(&mut self.rng, p));
        }
        if let Some(t0) = self.squash {
            dirty = true;
            if fx::squash(now.duration_since(t0)).is_none() {
                self.squash = None;
            }
        }
        let life_ms = self.pack.behaviour.life.sparkle_ms;
        if !self.glints.is_empty() {
            self.glints.retain(|g| g.scale(now, life_ms).is_some());
        }
        if let Some(at) = self.next_glint
            && at <= now
        {
            self.spawn_glints(now);
        }
        dirty |= self.step_teleport(now);
        if self.music.as_ref().and_then(|m| m.until).is_some_and(|u| u <= now) {
            self.music_off(now);
            dirty = true;
        }
        let pose = self.pose_at(now);
        if pose != self.pose_last {
            self.pose_last = pose;
            dirty = true;
        }

        // Window offset: the attention hop moves the window, no redraw.
        if let Some(at) = self.next_hop
            && at <= now
            && self.hop.is_none()
        {
            if self.hop_allowed() {
                self.hop = Some(now);
            }
            let p = self.spec(self.mood).and_then(|s| s.hop_every_ms).filter(|&p| p > 0);
            self.next_hop = p.map(|p| now + jitter(&mut self.rng, p));
        }
        if let Some(t0) = self.hop
            && fx::hop(now.duration_since(t0), 1.0).is_none()
        {
            self.hop = None;
        }
        self.apply_offset(now);
        self.update_notes(now);
        self.update_glints(now);
        if dirty {
            cx.notify();
        }
        self.schedule(cx);
    }

    fn spawn_glints(&mut self, now: Instant) {
        let [lo, hi] = self.pack.behaviour.life.sparkle_every_ms;
        if hi == 0 || self.pack.behaviour.life.sparkle_ms == 0 {
            self.next_glint = None;
            return;
        }
        let gap = (self.rng.range(lo as f32, hi.max(lo) as f32) as u64).max(250);
        self.next_glint = Some(now + Duration::from_millis(gap));
        if !self.loaded || self.hidden || self.teleport.is_some() {
            return;
        }
        let Some(f) = self.frame_or_default(self.anim.frame()) else { return };
        if f.glints.is_empty() {
            return;
        }
        let pts = f.glints.clone();
        let n = if self.rng.range(0.0, 1.0) < 0.3 { 2 } else { 1 };
        for k in 0..n {
            let (x, y) = pts[self.rng.pick(pts.len())];
            self.glints.push(Glint {
                t0: now + Duration::from_millis(k * 140),
                x: x as f32,
                y: y as f32,
                size: self.rng.range(7.0, 12.0) * self.layout.sf,
                spin: self.rng.range(-0.25, 0.25),
            });
        }
    }

    fn glitch_done(&mut self, now: Instant, cx: &mut Context<Self>) {
        if self.quitting {
            self.quit(cx);
            return;
        }
        let Some((leg, to)) = self.teleport.as_ref().map(|t| (t.leg, t.to)) else { return };
        match leg {
            Leg::Vanish => {
                self.hidden = true;
                let from = self.center_root(self.origin);
                let to = self.center_root(to);
                let seed = self.teleport.as_ref().map_or(0, |t| t.seed) ^ 0xb017_5eed;
                let bolt = Bolt::new(from, to, seed, 220.0 * self.layout.sf, self.bolt_style);
                if let Some(tp) = &mut self.teleport {
                    tp.bolt = Some(bolt);
                    tp.leg = Leg::Bolt(now);
                }
            }
            Leg::Appear => {
                self.teleport = None;
                if self.placement == Placement::Ground {
                    state::save(self.ground());
                }
            }
            Leg::Bolt(_) => {}
        }
    }

    /// The bolt leg of a teleport. True if the view must redraw.
    fn step_teleport(&mut self, now: Instant) -> bool {
        let Some(tp) = &mut self.teleport else { return false };
        let Leg::Bolt(t0) = tp.leg else { return false };
        let u = now.duration_since(t0).as_secs_f32() * 1000.0 / fx::BOLT_MS;
        let (to, place, moved) = (tp.to, tp.place, tp.moved);
        // The tail has all but reached the target: hand over to the landing glitch.
        if u >= fx::BOLT_END {
            for i in 0..BOLT_LAYERS {
                self.x11.fx_unmap(i);
            }
            self.x11.flush();
            tp.leg = Leg::Appear;
            self.hidden = false;
            self.glitch = Some(Glitch::new(GlitchKind::Land, now, tp.seed ^ 0x1a4d));
            if !moved {
                self.arrive(to, place, now);
            }
            return true;
        }
        if let Some(b) = &tp.bolt {
            let (bx, by, bw, bh) = b.bbox();
            let o = (bx, by);
            let layers = b.frame(u, (3.0 * self.layout.sf).round() as i32);
            // Top layer first, each lower one right under the one above it.
            for (i, layer) in layers.iter().enumerate().rev() {
                let rects = fx::polys_to_rects(&layer.polys, o, layer.grid);
                if i + 1 == layers.len() {
                    self.x11.fx_set(i, layer.rgb, o, (bw, bh), &rects).ok();
                } else {
                    self.x11.fx_set_below(i, i + 1, layer.rgb, o, (bw, bh), &rects).ok();
                }
            }
            for i in layers.len()..BOLT_LAYERS {
                self.x11.fx_unmap(i);
            }
            self.x11.flush();
        }
        if u >= fx::BOLT_ARRIVE && !moved {
            if let Some(tp) = &mut self.teleport {
                tp.moved = true;
            }
            self.arrive(to, place, now);
        }
        false
    }

    /// Land at the teleport target (window moved while the body is hidden).
    fn arrive(&mut self, to: (i32, i32), place: Placement, now: Instant) {
        let was_sitting = self.sitting();
        self.placement = place;
        self.set_origin(to);
        match place {
            Placement::Perch { win, .. } => self.x11.set_target(win),
            Placement::Ground => self.x11.set_target(0),
        }
        if was_sitting != self.sitting() {
            self.relayer(now);
        }
    }

    fn center_root(&self, o: (i32, i32)) -> (f32, f32) {
        let l = &self.layout;
        ((o.0 + l.avatar_x) as f32 + l.geo.width as f32 / 2.0, (o.1 + l.avatar_y) as f32 + l.geo.height as f32 * 0.55)
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
            Command::Pos(x, y, instant) => {
                let o = Self::clamp(&self.x11, &self.layout, (x - self.layout.ground_x, y - self.layout.ground_y));
                let (dx, dy) = (o.0 - self.origin.0, o.1 - self.origin.1);
                self.home = (o.0 + self.layout.ground_x, o.1 + self.layout.ground_y);
                if instant || !self.loaded || dx * dx + dy * dy < 48 * 48 {
                    self.teleport = None;
                    self.hidden = false;
                    self.fx_hide_bolt();
                    self.arrive(o, Placement::Ground, now);
                    state::save(self.ground());
                } else {
                    self.start_teleport(o, Placement::Ground, now);
                }
                format!("ok {} {}", self.home.0, self.home.1)
            }
            Command::Perch { sit, win } => match self.perch(sit, win, now) {
                Ok(r) => format!("ok perch 0x{:x} {} {} {}x{}", r.id, r.x, r.y, r.w, r.h),
                Err(e) => format!("error: {e}"),
            },
            Command::Unperch => {
                if matches!(self.placement, Placement::Perch { .. }) {
                    self.fall(now);
                    "ok".into()
                } else {
                    "error: not perched".into()
                }
            }
            Command::State => self.state_json(),
            Command::Hide => {
                self.hide(now);
                "bye".into()
            }
            Command::Quit => {
                self.quit(cx);
                "bye".into()
            }
            Command::Bolt { style, seed } => {
                self.bolt_style = style;
                self.bolt_seed = seed;
                match seed {
                    Some(n) => format!("ok bolt {} seed {n}", style.key()),
                    None => format!("ok bolt {}", style.key()),
                }
            }
            Command::Glitch => {
                self.glitch = Some(Glitch::new(GlitchKind::Big, now, self.rng.next()));
                "ok".into()
            }
            Command::Music(on) => {
                if on {
                    match self.music_on(now, None) {
                        Ok(()) => "ok".into(),
                        Err(e) => format!("error: {e}"),
                    }
                } else {
                    self.music_off(now);
                    "ok".into()
                }
            }
            Command::Page(d) => self.page(d, now),
            Command::Collapse(c) => match &mut self.bubble {
                Some(b) => {
                    b.collapsed = c;
                    if b.next_char.is_none() {
                        b.hide_at = Some(now + Duration::from_secs(10));
                    }
                    "ok".into()
                }
                None => "error: no bubble".into(),
            },
            Command::Watch(ev) => {
                self.watch(ev, now);
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
        self.bubble = Some(Bubble {
            text,
            shown: 0,
            per_tick,
            next_char: Some(now),
            hide_at: None,
            wrap: None,
            page: None,
            collapsed: false,
        });
        if self.mood != Mood::Talking {
            self.set_mood(Mood::Talking, now);
        } else {
            self.mood_then = None;
        }
    }

    fn page(&mut self, d: i32, now: Instant) -> String {
        let Some(b) = &mut self.bubble else { return "error: no bubble".into() };
        let (cur, total) = b.pages();
        let next = (cur as i32 + d).clamp(0, total as i32 - 1) as usize;
        b.page = Some(next);
        b.collapsed = false;
        if b.next_char.is_none() {
            b.hide_at = Some(now + Duration::from_secs(10));
        }
        format!("ok {}/{}", next + 1, total)
    }

    fn hide(&mut self, now: Instant) {
        self.menu = None;
        self.bubble = None;
        self.quitting = true;
        self.glitch = Some(Glitch::new(GlitchKind::Vanish, now, self.rng.next()));
    }

    fn quit(&mut self, cx: &mut Context<Self>) {
        if self.placement == Placement::Ground && self.teleport.is_none() {
            state::save(self.ground());
        }
        self.fx_hide_all();
        std::fs::remove_file(crate::control::socket_path()).ok();
        cx.quit();
    }

    fn state_json(&self) -> String {
        let frame = self.frame_or_default(self.anim.frame()).map(|f| f.id.clone()).unwrap_or_default();
        let (gx, gy) = self.ground();
        let m = self.x11.monitor_at(gx, gy - 1);
        let (page, pages) = self.bubble.as_ref().map_or((0, 0), |b| {
            let (p, n) = b.pages();
            (p + 1, n)
        });
        let placement = match self.placement {
            Placement::Ground => serde_json::json!({ "kind": "ground" }),
            Placement::Perch { win, sit, dx } => serde_json::json!({ "kind": if sit { "sit" } else { "stand" }, "win": win, "dx": dx }),
        };
        serde_json::json!({
            "mood": self.mood.key(),
            "monitor": { "x": m.x, "y": m.y, "w": m.w, "h": m.h },
            "frame": frame,
            "status": self.status().label,
            "window": { "x": self.origin.0, "y": self.origin.1, "w": self.layout.win_w, "h": self.layout.win_h },
            "ground": [gx, gy],
            "avatar": { "x": self.origin.0 + self.layout.avatar_x, "y": self.origin.1 + self.layout.avatar_y,
                        "w": self.layout.geo.width, "h": self.layout.geo.height },
            "bubble": self.bubble.as_ref().map(|b| b.text.iter().collect::<String>()),
            "page": [page, pages],
            "collapsed": self.bubble.as_ref().is_some_and(|b| b.collapsed),
            "placement": placement,
            "teleporting": self.teleport.is_some(),
            "glitch": self.glitch.is_some(),
            "composited": self.x11.composited(),
            "loaded": self.loaded,
            "scale_factor": self.layout.sf,
            "focus_probe": self.focus_probe,
            "music": self.music.is_some(),
            "bolt": self.bolt_style.key(),
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
        let gy = (y + l.ground_y).min(m.y + m.h - l.foot_pad - GROUND_ROOM);
        let top = gy - l.ground_y + l.avatar_y;
        let gy = if top < m.y { gy + (m.y - top) } else { gy };
        (ax - l.avatar_x, gy - l.ground_y)
    }

    pub fn default_origin(x11: &X11, l: &Layout) -> (i32, i32) {
        let m = x11.primary();
        let x = m.x + m.w - (l.avatar_x + l.geo.width as i32) - (24.0 * l.sf) as i32;
        let y = m.y + m.h - l.foot_pad - GROUND_ROOM - l.ground_y;
        (x, y)
    }

    /// Which part of the window is on the avatar's monitor; the bubble stays inside it.
    fn visible_range(x11: &X11, l: &Layout, o: (i32, i32)) -> (i32, i32) {
        let m = x11.monitor_at(o.0 + l.avatar_x + l.geo.width as i32 / 2, o.1 + l.avatar_y + l.geo.height as i32 / 2);
        ((m.x - o.0).max(0), (m.x + m.w - o.0).min(l.win_w))
    }

    /// Move the window (origin without offsets) and keep the bob/hop offset on top.
    fn set_origin(&mut self, o: (i32, i32)) {
        self.origin = o;
        self.vis = Self::visible_range(&self.x11, &self.layout, o);
        self.apply_offset(Instant::now());
    }

    fn move_to(&mut self, o: (i32, i32), persist: bool) {
        let o = Self::clamp(&self.x11, &self.layout, o);
        self.set_origin(o);
        if persist {
            state::save(self.ground());
        }
    }

    fn apply_offset(&mut self, now: Instant) {
        let hop = self.hop.and_then(|t| fx::hop(now.saturating_duration_since(t), 16.0 * self.layout.sf)).unwrap_or(0);
        let pos = (self.origin.0, self.origin.1 - hop);
        if pos != self.applied {
            self.x11.move_to(pos.0, pos.1).ok();
            self.applied = pos;
        }
    }

    fn start_teleport(&mut self, to: (i32, i32), place: Placement, now: Instant) {
        self.x11.set_target(0);
        self.menu = None;
        self.hop = None;
        if let Some(b) = &mut self.bubble {
            b.collapsed = true;
        }
        let seed = self.bolt_seed.unwrap_or_else(|| self.rng.next());
        self.glitch = Some(Glitch::new(GlitchKind::Vanish, now, seed));
        self.teleport = Some(Teleport { seed, leg: Leg::Vanish, to, place, bolt: None, moved: false });
    }

    /// Window origin that puts the seat (or the feet) on `r`'s top edge at `r.x + dx`.
    fn perch_origin(&self, r: &WinRect, sit: bool, dx: i32) -> (i32, i32) {
        let l = &self.layout;
        let x = r.x + dx;
        match (sit, l.seat_y) {
            (true, Some(sy)) => (x - l.ground_x, r.y - sy),
            _ => (x - l.ground_x, r.y - l.ground_y),
        }
    }

    /// What touches the edge line in the perch frame (seat, thighs, planted hands) and
    /// the legs hanging below it: x extent relative to the seat (or feet), device px.
    fn measure_contact(&self, sit: bool) -> Option<(i32, i32)> {
        let id = if sit { self.perch_spec()?.base.clone() } else { self.pack.catalog.default_sprite.clone() };
        self.contact_of(*self.pack.index.get(&id)?, sit)
    }

    fn contact_of(&self, fi: usize, sit: bool) -> Option<(i32, i32)> {
        let l = &self.layout;
        let f = self.frames.get(fi)?.as_ref()?;
        let line = if sit { l.seat_y? } else { l.ground_y } - l.avatar_y;
        let cx = l.ground_x - l.avatar_x;
        let (mut lo, mut hi) = (i32::MAX, i32::MIN);
        let from = if sit { line - (8.0 * l.sf) as i32 } else { line - (30.0 * l.sf) as i32 };
        for y in from..f.mask.h as i32 {
            for x in 0..f.mask.w as i32 {
                if f.mask.get(x, y) {
                    lo = lo.min(x);
                    hi = hi.max(x);
                }
            }
        }
        (lo <= hi).then_some((lo - cx, hi - cx))
    }

    /// Where the seat/feet may go along `r`'s top edge so that everything touching it is
    /// on the window, EDGE_MARGIN from both corners. None: the window is too narrow.
    fn perch_range(&self, r: &WinRect, sit: bool) -> Option<(i32, i32)> {
        let m = (EDGE_MARGIN * self.layout.sf).round() as i32;
        let (l, h) = self.contact[sit as usize].unwrap_or((-m, m));
        let (lo, hi) = (m - l, r.w - m - h);
        (lo <= hi).then_some((lo, hi))
    }

    /// The seat/feet x on `r` (relative to its left edge): `want`, or PERCH_AT across by
    /// default, clamped into the safe range.
    fn perch_dx(&self, r: &WinRect, sit: bool, want: Option<i32>) -> Option<i32> {
        let (lo, hi) = self.perch_range(r, sit)?;
        Some(want.unwrap_or((r.w as f32 * PERCH_AT) as i32).clamp(lo, hi))
    }

    /// Whether a perch on `r` keeps the figure on `r`'s monitor and below its top.
    fn perch_fits(&self, r: &WinRect, o: (i32, i32), sit: bool) -> bool {
        let m = self.x11.monitor_at(r.x + r.w / 2, r.y + 1);
        if r.x == m.x && r.y == m.y && r.w == m.w && r.h == m.h {
            return false; // fullscreen
        }
        let base = if sit { self.perch_spec().map(|p| p.base.clone()) } else { None }.unwrap_or_else(|| self.pack.catalog.default_sprite.clone());
        let top = self.pack.index.get(&base).and_then(|&i| self.frames[i].as_ref()).map_or(0, |f| f.top);
        o.1 + self.layout.avatar_y + top >= m.y && r.y > m.y
    }

    fn perch(&mut self, sit: bool, win: Option<u32>, now: Instant) -> anyhow::Result<WinRect> {
        let sit = sit && self.layout.seat_y.is_some();
        let id = match win {
            Some(w) => w,
            None => self.x11.active_window().ok_or_else(|| anyhow::anyhow!("no focused window (_NET_ACTIVE_WINDOW)"))?,
        };
        let top = self.x11.toplevel_of(id).ok_or_else(|| anyhow::anyhow!("window 0x{id:x} not found"))?;
        if top == self.x11.win {
            anyhow::bail!("that's me");
        }
        let r = self.x11.toplevel_rect(top).ok_or_else(|| anyhow::anyhow!("window 0x{top:x} is not visible"))?;
        let dx = self.perch_dx(&r, sit, None).ok_or_else(|| anyhow::anyhow!("window 0x{top:x} is too narrow to sit on"))?;
        let o = self.perch_origin(&r, sit, dx);
        if !self.perch_fits(&r, o, sit) {
            anyhow::bail!("no room above window 0x{top:x} (or it is fullscreen)");
        }
        if self.placement == Placement::Ground {
            self.home = self.ground();
        }
        self.start_teleport(o, Placement::Perch { win: top, sit, dx }, now);
        Ok(r)
    }

    /// The perch window is gone, fullscreen or out of room: teleport back to the ground.
    fn fall(&mut self, now: Instant) {
        self.x11.set_target(0);
        let o = Self::clamp(&self.x11, &self.layout, (self.home.0 - self.layout.ground_x, self.home.1 - self.layout.ground_y));
        self.start_teleport(o, Placement::Ground, now);
    }

    fn watch(&mut self, ev: WatchEvent, now: Instant) {
        match ev {
            WatchEvent::Compositor(c) => {
                eprintln!("synaps-avatar: compositor {}", if c { "appeared" } else { "went away" });
                self.last_shape.borrow_mut().0.clear();
            }
            WatchEvent::TargetMoved(r) => {
                let Placement::Perch { win, sit, dx } = self.placement else { return };
                if win != r.id || self.teleport.is_some() {
                    return;
                }
                let Some(dx) = self.perch_dx(&r, sit, Some(dx)) else {
                    self.fall(now);
                    return;
                };
                let o = self.perch_origin(&r, sit, dx);
                if self.perch_fits(&r, o, sit) {
                    self.placement = Placement::Perch { win, sit, dx };
                    self.set_origin(o);
                } else {
                    self.fall(now);
                }
            }
            WatchEvent::TargetGone(id) => {
                if matches!(self.placement, Placement::Perch { win, .. } if win == id) {
                    self.fall(now);
                }
            }
        }
    }

    /// After a drag: a window whose top edge is near the seat or the feet.
    fn snap_target(&self) -> Option<(WinRect, i32)> {
        let l = &self.layout;
        let (gx, gy) = self.ground();
        let sy = l.seat_y.map_or(gy, |s| self.origin.1 + s);
        let tol = (36.0 * l.sf) as i32;
        self.x11.toplevels().into_iter().rev().find_map(|r| {
            let inside = gx > r.x + 20 && gx < r.x + r.w - 20;
            let near = (sy - r.y).abs() < tol || (gy - r.y).abs() < tol;
            let dx = self.perch_dx(&r, true, Some(gx - r.x))?;
            (inside && near && self.perch_fits(&r, self.perch_origin(&r, true, dx), true)).then_some((r, dx))
        })
    }

    // ---- pointer --------------------------------------------------------------------

    fn local(&self, p: Point<Pixels>) -> (i32, i32) {
        let sf = self.layout.sf;
        ((f32::from(p.x) * sf) as i32 - self.layout.avatar_x, (f32::from(p.y) * sf) as i32 - self.layout.avatar_y)
    }

    fn hit(&self, p: Point<Pixels>) -> Option<bool> {
        if self.hidden || self.teleport.is_some() {
            return None;
        }
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

    fn mouse_move(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        if ev.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let Some(p) = &mut self.press else { return };
        let Ok(root) = self.x11.pointer() else { return };
        let (dx, dy) = (root.0 - p.root.0, root.1 - p.root.1);
        if !p.dragging && dx * dx + dy * dy < 25 {
            return;
        }
        let o = (p.origin.0 + dx, p.origin.1 + dy);
        if !p.dragging {
            p.dragging = true;
            // Picked up off a perch: stand again while carried.
            if matches!(self.placement, Placement::Perch { .. }) {
                let was_sitting = self.sitting();
                self.placement = Placement::Ground;
                self.x11.set_target(0);
                if was_sitting {
                    self.relayer(Instant::now());
                    cx.notify();
                }
            }
        }
        self.move_to(o, false);
    }

    fn mouse_up(&mut self, ev: &MouseUpEvent, cx: &mut Context<Self>) {
        if ev.button != MouseButton::Left {
            return;
        }
        let Some(p) = self.press.take() else { return };
        let now = Instant::now();
        if p.dragging {
            if let Some((r, dx)) = self.snap_target() {
                self.home = self.ground();
                let o = self.perch_origin(&r, true, dx);
                self.placement = Placement::Perch { win: r.id, sit: self.layout.seat_y.is_some(), dx };
                self.set_origin(o);
                self.x11.set_target(r.id);
                self.relayer(now);
                self.glitch = Some(Glitch::new(GlitchKind::Burst, now, self.rng.next()));
            } else {
                self.home = self.ground();
                state::save(self.ground());
            }
            cx.notify();
            self.schedule(cx);
            return;
        }
        self.streak = if now.duration_since(self.streak.1) < Duration::from_millis(self.pack.behaviour.click.streak_ms) {
            (self.streak.0 + 1, now)
        } else {
            (1, now)
        };
        let click = self.pack.behaviour.click.clone();
        let perch_click = self.perch_spec().filter(|_| self.sitting()).map(|p| p.click.clone());
        let steps = if let Some(steps) = perch_click {
            if p.head {
                self.hearts = Some((now, ev.position));
            }
            steps
        } else if self.streak.0 >= click.annoyed_after && !click.annoyed.is_empty() {
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
        let len: u64 = seq.iter().map(|s| s.1 as u64).sum();
        self.anim.play(seq, now);
        self.squash = Some(now);
        let hold = if self.hearts.is_some() { len.max(HEARTS_MS) } else { len.max(600) };
        self.reaction_until = Some(now + Duration::from_millis(hold));
        if self.next_activity.is_some() {
            self.next_activity = Some(now + self.autonomy_gap());
        }
        cx.notify();
        self.schedule(cx);
    }

    fn reset_position(&mut self, cx: &mut Context<Self>) {
        self.menu = None;
        let now = Instant::now();
        let o = Self::default_origin(&self.x11, &self.layout);
        self.home = (o.0 + self.layout.ground_x, o.1 + self.layout.ground_y);
        if self.loaded {
            self.start_teleport(o, Placement::Ground, now);
        } else {
            self.move_to(o, true);
        }
        cx.notify();
        self.schedule(cx);
    }

    /// Squash pivot: the seat when sitting on an edge, else the feet (avatar-local).
    fn pivot(&self) -> (f32, f32) {
        let l = &self.layout;
        let y = match (self.sitting(), l.seat_y) {
            (true, Some(s)) => s,
            _ => l.ground_y,
        };
        ((l.ground_x - l.avatar_x) as f32, (y - l.avatar_y) as f32)
    }

    /// Laid-out width of `tracked(s, spacing)` at `size`/`weight` (logical px).
    fn tracked_width(&self, s: &str, size: f32, weight: FontWeight, spacing: f32, window: &Window) -> f32 {
        if s.is_empty() {
            return 0.0;
        }
        // `tracked` lays out one box per glyph: no kerning, each box rounded up.
        let f = Font { weight, ..font(self.font.clone()) };
        s.chars()
            .map(|c| {
                let t = if c == ' ' { "\u{00A0}".to_string() } else { c.to_string() };
                let run = TextRun { len: t.len(), font: f.clone(), color: black(), background_color: None, underline: None, strikethrough: None };
                f32::from(window.text_system().layout_line(&t, px(size), &[run], None).width).ceil() + spacing
            })
            .sum()
    }

    fn wrap_text(&self, text: &[char], window: &Window, cx: &App) -> Wrapped {
        let f = Font { weight: FontWeight::SEMIBOLD, ..font(self.font.clone()) };
        let wrap_w = PANEL_MAX_W - PAD_L - PAD_R;
        let mut wrapper = cx.text_system().line_wrapper(f.clone(), px(BODY_FONT));
        let s: String = text.iter().collect();
        let mut lines = Vec::new();
        let mut base = 0usize;
        for para in s.split('\n') {
            let breaks: Vec<usize> = wrapper
                .wrap_line(&[LineFragment::text(para)], px(wrap_w), IndentAdjustment::NoIndent)
                .map(|b| b.ix)
                .collect();
            let mut start = 0usize;
            for ix in breaks.into_iter().chain(std::iter::once(para.len())) {
                lines.push((base + para[..start].chars().count(), base + para[..ix].chars().count()));
                start = ix;
            }
            base += para.chars().count() + 1;
        }
        let width = lines
            .iter()
            .map(|&(a, b)| {
                let t: String = text[a..b.min(text.len())].iter().collect();
                let t = t.trim_end();
                if t.is_empty() {
                    return 0.0;
                }
                let run = TextRun { len: t.len(), font: f.clone(), color: black(), background_color: None, underline: None, strikethrough: None };
                f32::from(window.text_system().layout_line(t, px(BODY_FONT), &[run], None).width)
            })
            .fold(0.0, f32::max);
        Wrapped { lines, width }
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

#[derive(Clone, Copy)]
enum Chevron {
    Left,
    Right,
    Down,
    Up,
}

/// A small stroked chevron (Capri's reply controls draw theirs as SVG strokes).
fn chevron(dir: Chevron, colour: Rgba) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |b, _, window, _| {
            let (x, y) = (f32::from(b.origin.x), f32::from(b.origin.y));
            let pts: [(f32, f32); 3] = match dir {
                Chevron::Left => [(6.5, 1.5), (2.5, 5.0), (6.5, 8.5)],
                Chevron::Right => [(3.5, 1.5), (7.5, 5.0), (3.5, 8.5)],
                Chevron::Down => [(1.5, 3.0), (5.0, 7.0), (8.5, 3.0)],
                Chevron::Up => [(1.5, 7.0), (5.0, 3.0), (8.5, 7.0)],
            };
            let mut p = PathBuilder::stroke(px(1.8));
            p.move_to(point(px(x + pts[0].0), px(y + pts[0].1)));
            p.line_to(point(px(x + pts[1].0), px(y + pts[1].1)));
            p.line_to(point(px(x + pts[2].0), px(y + pts[2].1)));
            if let Ok(path) = p.build() {
                window.paint_path(path, colour);
            }
        },
    )
    .w(px(10.0))
    .h(px(10.0))
}

struct Status {
    label: String,
    tone: u32,
    invert: bool,
    glyph: String,
}

/// A small status glyph drawn as shapes (crisp at any size, no font fallback). 11 px box;
/// `bg` paints cut-outs.
fn glyph(kind: &str, fg: u32, bg: u32) -> impl IntoElement {
    let kind = kind.to_string();
    canvas(
        |_, _, _| (),
        move |b, _, window, _| {
            let (ox, oy) = (f32::from(b.origin.x), f32::from(b.origin.y));
            let p = |x: f32, y: f32| point(px(ox + x), px(oy + y));
            let fill_poly = |window: &mut Window, pts: &[(f32, f32)], c: u32| {
                let mut pb = PathBuilder::fill();
                pb.add_polygon(&pts.iter().map(|&(x, y)| p(x, y)).collect::<Vec<_>>(), true);
                if let Ok(path) = pb.build() {
                    window.paint_path(path, rgb(c));
                }
            };
            let circle = |cx: f32, cy: f32, r: f32| (0..20).map(|i| {
                let a = i as f32 / 20.0 * std::f32::consts::TAU;
                (cx + r * a.cos(), cy + r * a.sin())
            }).collect::<Vec<_>>();
            let stroke = |window: &mut Window, pts: &[(f32, f32)], w: f32, c: u32| {
                let mut pb = PathBuilder::stroke(px(w));
                pb.move_to(p(pts[0].0, pts[0].1));
                for &(x, y) in &pts[1..] {
                    pb.line_to(p(x, y));
                }
                if let Ok(path) = pb.build() {
                    window.paint_path(path, rgb(c));
                }
            };
            match kind.as_str() {
                "ring" => {
                    fill_poly(window, &circle(5.5, 5.5, 4.6), fg);
                    fill_poly(window, &circle(5.5, 5.5, 2.6), bg);
                }
                "gear" => {
                    let pts: Vec<(f32, f32)> = (0..32)
                        .map(|i| {
                            let a = (i as f32 + 0.5) / 32.0 * std::f32::consts::TAU;
                            let r = if (i / 2) % 2 == 0 { 5.6 } else { 4.1 };
                            (5.5 + r * a.cos(), 5.5 + r * a.sin())
                        })
                        .collect();
                    fill_poly(window, &pts, fg);
                    fill_poly(window, &circle(5.5, 5.5, 1.9), bg);
                }
                "play" => fill_poly(window, &[(2.0, 1.0), (10.0, 5.5), (2.0, 10.0)], fg),
                "check" => stroke(window, &[(1.2, 5.8), (4.3, 8.9), (10.0, 2.0)], 2.2, fg),
                "bang" => {
                    fill_poly(window, &[(4.4, 0.5), (6.6, 0.5), (6.2, 7.0), (4.8, 7.0)], fg);
                    fill_poly(window, &[(4.5, 8.4), (6.5, 8.4), (6.5, 10.5), (4.5, 10.5)], fg);
                }
                "cross" => {
                    stroke(window, &[(1.5, 1.5), (9.5, 9.5)], 2.2, fg);
                    stroke(window, &[(9.5, 1.5), (1.5, 9.5)], 2.2, fg);
                }
                "moon" => {
                    fill_poly(window, &circle(5.2, 5.5, 4.8), fg);
                    fill_poly(window, &circle(7.6, 3.9, 3.9), bg);
                }
                "dots" => {
                    for x in [0.8, 4.3, 7.8] {
                        fill_poly(window, &[(x, 6.5), (x + 2.4, 6.5), (x + 2.4, 8.9), (x, 8.9)], fg);
                    }
                }
                "note" => {
                    fill_poly(window, &circle(3.2, 8.3, 2.4), fg);
                    fill_poly(window, &[(4.4, 1.0), (5.8, 1.0), (5.8, 8.3), (4.4, 8.3)], fg);
                    fill_poly(window, &[(5.8, 1.0), (9.8, 3.2), (9.8, 5.0), (5.8, 2.9)], fg);
                }
                _ => fill_poly(window, &circle(5.5, 5.5, 4.0), fg),
            }
        },
    )
    .flex_none()
    .w(px(11.0))
    .h(px(11.0))
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

/// Bubble panel geometry, logical px in window coordinates.
#[derive(Clone, Copy)]
struct PanelGeom {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    tail_x: f32,
    tip: (f32, f32),
}

impl PanelGeom {
    /// The outline: cut corners top-left and bottom-right, the tail on the bottom edge.
    fn poly(&self) -> Vec<(f32, f32)> {
        let (x, y, w, h, c) = (self.x, self.y, self.w, self.h, CUT);
        let b = y + h;
        let half = TAIL_W / 2.0;
        vec![
            (x + c, y),
            (x + w, y),
            (x + w, b - c),
            (x + w - c, b),
            (self.tail_x + half, b),
            self.tip,
            (self.tail_x - half, b),
            (x, b),
            (x, y + c),
        ]
    }
}

const HEARTS: [(f32, f32, f32, u32); 4] = [
    // dx, size, delay ms, colour
    (-14.0, 36.0, 0.0, 0xff3e83),
    (32.0, 28.0, 90.0, 0xff79b8),
    (-48.0, 23.0, 170.0, 0xff1f63),
    (12.0, 18.0, 250.0, 0xffb3d9),
];

impl Render for Avatar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        let l = self.layout;
        let sf = l.sf;
        let pal = Palette {
            frame: self.pack.tone_or("frame", 0x8ef5ed),
            accent: self.pack.tone_or("accent", 0xc6fcf0),
            surface: self.pack.tone_or("surface", 0x0b131a),
            text: self.pack.tone_or("text", 0xf1f7f9),
            dim: self.pack.tone_or("dim", 0xc4d6df),
            name: self.pack.tone_or("name", 0xf4ed75),
            slash: self.pack.tone_or("slash", 0xff3c68),
        };
        if let Some(b) = &self.bubble
            && b.wrap.is_none()
        {
            let w = self.wrap_text(&b.text, window, cx);
            if let Some(b) = &mut self.bubble {
                b.wrap = Some(w);
            }
        }
        let frame = self.frame_or_default(self.anim.frame());
        // HUD placement follows the mood's resting frame, so loops don't make it jitter.
        let base = self.frame_or_default(self.anim.base()).or(frame);
        // …but a taller one-shot (an entry clip) pushes it up so it never sits on the hair.
        let (base_top, head_x) = base.map_or((0, l.geo.width as i32 / 2), |f| (f.top.min(frame.map_or(f.top, |c| c.top)), f.head_x));
        let top_l = (l.avatar_y + base_top) as f32 / sf;
        let head_l = (l.avatar_x + head_x) as f32 / sf;
        // The status tag sits just clear of the hair under the tag itself (this frame and
        // the resting one), not of the whole silhouette.
        let tag_hair = {
            let tw = self.tag_w.get();
            let (c0, c1) = (((head_l - tw / 2.0) * sf) as i32 - l.avatar_x - 4, ((head_l + tw / 2.0) * sf) as i32 - l.avatar_x + 4);
            let under = |f: Option<&Frame>| {
                f.and_then(|f| (c0.max(0)..c1.min(f.profile.len() as i32)).map(|x| f.profile[x as usize]).min()).filter(|&v| v != i16::MAX).map(i32::from)
            };
            under(base).into_iter().chain(under(frame)).min().unwrap_or(base_top)
        };
        let image = frame.map(|f| f.image.clone());
        let rects = frame.map(|f| f.rects.clone()).unwrap_or_default();
        let hidden = self.hidden || frame.is_none();
        let has_frame = frame.is_some() && !hidden;
        let (aw, ah) = (l.geo.width as i32, l.geo.height as i32);

        // Body plan: squash, glitch.
        let mut plan = BodyPlan::plain();
        plan.pivot = self.pivot();
        if let Some(t0) = self.squash
            && let Some((sx, sy)) = fx::squash(now.saturating_duration_since(t0))
        {
            plan.sx = sx;
            plan.sy = sy;
        }
        if let Some(g) = self.glitch {
            let (top, face, bx0, bx1) = frame.map_or((0, None, 0, aw), |f| {
                let (x0, x1) = f.profile.iter().enumerate().filter(|p| *p.1 != i16::MAX).fold((aw, 0), |a, (i, _)| (a.0.min(i as i32), a.1.max(i as i32)));
                (f.top, g.keeps_face().then_some(f.face), x0, x1.max(x0 + 1))
            });
            plan.glitch(fx::GlitchStep {
                strength: g.strength(now),
                coverage: g.coverage(now),
                seed: g.step_seed(now),
                order: g.seed(),
                y0: -l.avatar_y,
                h: ah,
                body: (bx0, top, bx1 - bx0, ah - top),
                face,
                solid: g.solid(now),
                sf,
            });
            plan.tint = g.tint(now);
        }
        if plan.slices.is_none() && plan.sx == 1.0 && plan.sy == 1.0 && !hidden {
            let pose = self.pose_at(now);
            let fi = self.anim.frame();
            if pose != fx::Pose::default()
                && let Some(f) = self.frames.get(fi).and_then(|f| f.as_ref())
            {
                plan.slices = Some(fx::pose_bands(pose, self.pose_rows(fi, f)));
            }
        }
        let plan = Rc::new(plan);
        let body_shape: Vec<ShapeRect> = if hidden {
            Vec::new()
        } else if plan.is_plain() {
            rects.to_vec()
        } else {
            plan.shape(&rects)
        };
        let ghost_rects = if !plan.has_ghosts() || hidden { Vec::new() } else { plan.ghost_rects(&rects) };
        let solid_rects = if plan.solid.iter().all(Option::is_none) || hidden { Vec::new() } else { plan.solid_rects(&rects) };
        let tint_rects = if plan.tint.is_some() && !hidden { plan.body_rects(&rects) } else { Vec::new() };
        let entity = cx.entity();
        let warm: Vec<Arc<RenderImage>> = if self.loaded && !self.warmed {
            self.warmed = true;
            self.frames.iter().flatten().map(|f| f.image.clone()).collect()
        } else {
            Vec::new()
        };

        let menu_cell: Cell2 = Rc::new(Cell::new(None));
        let tag_cell: Cell2 = Rc::new(Cell::new(None));
        let mut root = div().size_full().relative().font_family(self.font.clone()).text_color(rgb(pal.text));

        // The body.
        let plan2 = plan.clone();
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
                    let (ax, ay) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
                    let lp = |v: f32| v / sf;
                    let rect = |x: f32, y: f32, w: f32, h: f32| Bounds::new(point(px(ax + lp(x)), px(ay + lp(y))), size(px(lp(w)), px(lp(h))));
                    if let (Some(img), false) = (image, hidden) {
                        for &(r, c) in &ghost_rects {
                            window.paint_quad(fill(rect(r.0 as f32, r.1 as f32, r.2 as f32, r.3 as f32), rgb(c)));
                        }
                        let (ix, iy, iw, ih) = plan2.image_rect(aw, ah);
                        let ib = rect(ix, iy, iw, ih);
                        match &plan2.slices {
                            _ if plan2.tint.is_some() => {
                                let c = rgb(plan2.tint.unwrap_or(fx::BOLT_RED));
                                for r in &tint_rects {
                                    window.paint_quad(fill(rect(r.0 as f32, r.1 as f32, r.2 as f32, r.3 as f32), c));
                                }
                            }
                            None => {
                                // Squashed: the unsquashed frame goes underneath. Outside the
                                // (squashed) bounding shape it's invisible, but it fills the
                                // window's backing pixels there, so the band the shape grows
                                // back into on the rebound shows hair, not stale black.
                                if plan2.sy < 1.0 || plan2.sx < 1.0 {
                                    let full = rect(0.0, 0.0, aw as f32, ah as f32);
                                    window.paint_image(full, full, Corners::default(), img.clone(), 0, false).ok();
                                }
                                window.paint_image(ib, ib, Corners::default(), img, 0, false).ok();
                            }
                            Some(bands) => {
                                for (r, c) in &solid_rects {
                                    window.paint_quad(fill(rect(r.0 as f32, r.1 as f32, r.2 as f32, r.3 as f32), rgb(*c)));
                                }
                                for (i, &(y0, y1, dx, dy)) in bands.iter().enumerate() {
                                    if plan2.solid.get(i).is_some_and(Option::is_some) {
                                        continue;
                                    }
                                    let shifted = rect(ix + dx as f32, iy + dy as f32, iw, ih);
                                    let clip = rect(ix + dx as f32, (y0 + dy) as f32, iw, (y1 - y0) as f32);
                                    window.paint_image(clip, shifted, Corners::default(), img.clone(), 0, false).ok();
                                }
                            }
                        }
                        if plan2.scan > 0.02 {
                            let (dark, light) = (with_alpha(0x02040a, plan2.scan), with_alpha(0x02040a, plan2.scan * 0.4));
                            let in_face = |y: i32| plan2.face.is_some_and(|(a, b)| y >= a && y < b);
                            let mut y = 0;
                            while y < ah {
                                let c = if in_face(y) { light } else { dark };
                                window.paint_quad(fill(rect(-120.0, y as f32, aw as f32 + 240.0, (1.0 * sf).max(1.0)), c));
                                y += (3.0 * sf).round() as i32;
                            }
                        }
                        for &(r, c) in &plan2.cells {
                            window.paint_quad(fill(rect(r.0 as f32, r.1 as f32, r.2 as f32, r.3 as f32), rgb(c)));
                        }
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
            .w(l.lp(aw))
            .h(l.lp(ah)),
        );

        let hud = !hidden && self.teleport.is_none() && !self.quitting;
        let Status { label, tone, invert, glyph: glyph_kind } = self.status();
        let bubble_open = self.focus_probe.is_some() || self.bubble.as_ref().is_some_and(|b| !b.collapsed);
        let collapsed = self.bubble.as_ref().is_some_and(|b| b.collapsed);

        // Status tag above the hair, unless the bubble carries the status or a reaction
        // needs the space.
        let show_tag = hud && self.frames.iter().any(Option::is_some) && !bubble_open && self.reaction_until.is_none();
        // In a glitch the tag tears with the body: strips of it, shifted by the same bands.
        let mut tag_strips: Vec<ShapeRect> = Vec::new();
        let torn = show_tag && !collapsed && self.glitch.is_some();
        if show_tag {
            let (bg, fg, border) = if invert { (tone, pal.surface, tone) } else { (pal.surface, tone, tone) };
            let text = format!("{} // {}", self.name, label);
            let make_ring = || {
                let plate = div()
                    .flex()
                    .items_center()
                    .gap(px(7.0))
                    .pl(px(8.0))
                    .pr(px(10.0))
                    .py(px(5.0))
                    .border_1()
                    .border_color(rgb(border))
                    .bg(rgb(bg))
                    .text_color(rgb(fg))
                    .text_size(px(TAG_FONT))
                    .line_height(px(TAG_FONT + 1.0))
                    .font_weight(FontWeight::BOLD)
                    .child(glyph(&glyph_kind, fg, bg))
                    .child(tracked(&text, 1.5));
                // A dark ring keeps the plate legible on busy backgrounds.
                div().p(px(1.0)).bg(rgb(0x02060a)).child(plate)
            };
            if let (true, Some(b), Some(bands)) = (torn, self.tag_box.get(), plan.slices.as_ref()) {
                let (bx, by) = (f32::from(b.origin.x), f32::from(b.origin.y));
                let (bw, bh) = (f32::from(b.size.width), f32::from(b.size.height));
                let t0 = by * sf - l.avatar_y as f32;
                for &(y0, y1, dx, _) in bands {
                    let (a, c) = ((y0 as f32).max(t0), (y1 as f32).min(t0 + bh * sf));
                    if c <= a {
                        continue;
                    }
                    let (s0, s1, dx) = ((a - t0) / sf, (c - t0) / sf, dx as f32 / sf);
                    tag_strips.push(to_device(Bounds::new(point(px(bx + dx), px(by + s0)), size(px(bw), px(s1 - s0))), sf));
                    root = root.child(
                        div()
                            .absolute()
                            .left(px(bx + dx))
                            .top(px(by + s0))
                            .w(px(bw))
                            .h(px(s1 - s0))
                            .overflow_hidden()
                            .child(div().absolute().left_0().top(px(-s0)).w(px(bw)).h(px(bh)).child(make_ring())),
                    );
                }
            }
            let ring = make_ring();
            let mut row = div().id("tag").flex().items_end().gap(px(3.0));
            if collapsed {
                row = row
                    .child(
                        div()
                            .p(px(1.0))
                            .bg(rgb(0x02060a))
                            .child(div().w(px(20.0)).h(px(19.0)).flex().items_center().justify_center().border_1().border_color(rgb(tone)).bg(rgb(pal.surface)).child(chevron(Chevron::Up, rgb(tone)))),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            if let Some(b) = &mut this.bubble {
                                b.collapsed = false;
                                if b.next_char.is_none() {
                                    b.hide_at = Some(Instant::now() + Duration::from_secs(10));
                                }
                            }
                            cx.notify();
                            this.schedule(cx);
                        }),
                    );
            }
            row = row.child(ring);
            let centre = head_l;
            let tag_bottom = (l.avatar_y + tag_hair) as f32 / sf - TAG_GAP;
            let mut holder = div()
                .absolute()
                .left_0()
                .w(l.lp(l.win_w))
                .bottom(px(l.win_h as f32 / sf - tag_bottom))
                .flex()
                .justify_center()
                .pl(px((2.0 * centre - l.win_w as f32 / sf).max(0.0)))
                .pr(px((l.win_w as f32 / sf - 2.0 * centre).max(0.0)));
            // Torn: the whole tag still lays out (invisibly) so its box stays current.
            if torn {
                holder = holder.invisible();
            }
            root = root.child(holder.child(probed(row, &tag_cell)));
        }

        // Speech bubble (or the focus-probe input).
        let mut panel_geom: Option<PanelGeom> = None;
        let content: Option<(Vec<String>, Option<(usize, usize)>, bool)> = if !hud {
            None
        } else if let Some(text) = &self.focus_probe {
            let body = if text.is_empty() { "click here, then type".to_string() } else { text.clone() };
            Some((vec![body], None, true))
        } else {
            self.bubble.as_ref().filter(|b| !b.collapsed).map(|b| (b.page_lines(), Some(b.pages()), false))
        };
        if let Some((lines, pages, probe)) = content {
            let (vis_l, vis_r) = (self.vis.0 as f32 / sf, self.vis.1 as f32 / sf);
            let max_w = (vis_r - vis_l - 2.0 * MARGIN).min(PANEL_MAX_W);
            let text_w = self.bubble.as_ref().and_then(|b| b.wrap.as_ref()).map_or(260.0, |w| w.width);
            // Never narrower than the header line (name / label .. glyph status).
            let header_label = if probe { "FOCUS PROBE // CLICK ME" } else { "INCOMING TRANSMISSION" };
            let header_w = PAD_L
                + PAD_R
                + self.tracked_width(&self.name, 10.5, FontWeight::BOLD, 1.8, window)
                + 4.0
                + self.tracked_width("/", 10.5, FontWeight::BOLD, 0.0, window)
                + self.tracked_width(header_label, 9.5, FontWeight::SEMIBOLD, 1.5, window)
                + self.tracked_width(&label, 9.5, FontWeight::BOLD, 1.5, window)
                + 3.0 * 8.0
                + 11.0
                + 5.0
                + 32.0;
            let min_w = PANEL_MIN_W.max(header_w).min(max_w);
            let w = (text_w + PAD_L + PAD_R + 2.0).clamp(min_w, max_w);
            let total_lines = self.bubble.as_ref().and_then(|b| b.wrap.as_ref()).map_or(1, |w| w.lines.len());
            // Fit the page: the lines it will hold once typed out (so it doesn't grow while
            // streaming), never an empty band above the footer.
            let rows = match pages {
                Some((cur, n)) if n > 1 => total_lines.saturating_sub(cur * LINES_PER_PAGE).clamp(1, LINES_PER_PAGE),
                _ if probe => 1,
                _ => total_lines.clamp(1, LINES_PER_PAGE),
            };
            let footer = !probe;
            let h = PAD_T + HEADER_H + HEADER_GAP + rows as f32 * LINE_H + if footer { FOOTER_GAP + FOOTER_H } else { 0.0 } + PAD_B;
            let x = (head_l - w * 0.45).clamp(vis_l + MARGIN, (vis_r - MARGIN - w).max(vis_l + MARGIN));
            let bottom = top_l - TIP_GAP - TAIL_H;
            let y = (bottom - h).max(TOP_MARGIN);
            let lo = x + CUT + TAIL_W / 2.0 + 6.0;
            let hi = x + w - CUT - TAIL_W / 2.0 - 6.0;
            let tail_x = head_l.clamp(lo, hi.max(lo));
            let g = PanelGeom { x, y, w, h, tail_x, tip: (head_l.clamp(tail_x - 9.0, tail_x + 9.0), y + h + TAIL_H) };
            panel_geom = Some(g);
            let (frame_c, accent_c, bar_c, surface_c) = (pal.frame, pal.accent, pal.slash, pal.surface);
            let bg = canvas(
                |_, _, _| (),
                move |_, _, window, _| {
                    let pts: Vec<Point<Pixels>> = g.poly().iter().map(|&(x, y)| point(px(x), px(y))).collect();
                    let mut surface = PathBuilder::fill();
                    surface.add_polygon(&pts, true);
                    if let Ok(p) = surface.build() {
                        window.paint_path(p, rgb(surface_c));
                    }
                    // Centred on the edge: the outer half falls outside the X shape, so
                    // 1.5 px shows.
                    let mut edge = PathBuilder::stroke(px(3.0));
                    edge.add_polygon(&pts, true);
                    if let Ok(p) = edge.build() {
                        window.paint_path(p, with_alpha(frame_c, 0.85));
                    }
                    let x0 = g.x + CUT + 27.0;
                    window.paint_quad(fill(Bounds::new(point(px(x0), px(g.y)), size(px(42.0), px(2.0))), rgb(accent_c)));
                    window.paint_quad(fill(Bounds::new(point(px(g.x + 8.0), px(g.y + 17.0)), size(px(2.0), px(g.h - 30.0))), rgb(bar_c)));
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full();
            let header_label = if probe {
                if self.focus.is_focused(window) { "FOCUS PROBE // KEYS OK" } else { "FOCUS PROBE // CLICK ME" }
            } else {
                "INCOMING TRANSMISSION"
            };
            let header = div()
                .absolute()
                .top(px(PAD_T))
                .left(px(PAD_L))
                .right(px(PAD_R))
                .h(px(HEADER_H))
                .flex()
                .items_center()
                .gap(px(8.0))
                .line_height(px(HEADER_H))
                .child(
                    div()
                        .flex()
                        .text_size(px(10.5))
                        .font_weight(FontWeight::BOLD)
                        .child(tracked(&self.name, 1.8).text_color(rgb(pal.name)))
                        .child(div().ml(px(4.0)).text_color(rgb(pal.slash)).child("/")),
                )
                .child(tracked(header_label, 1.5).text_size(px(9.5)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(pal.dim)))
                .child(div().flex_1())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(5.0))
                        .child(glyph(&glyph_kind, tone, pal.surface))
                        .child(tracked(&label, 1.5).text_size(px(9.5)).font_weight(FontWeight::BOLD).text_color(rgb(tone))),
                );
            let body = div()
                .absolute()
                .top(px(PAD_T + HEADER_H + HEADER_GAP))
                .left(px(PAD_L))
                .right(px(PAD_R))
                .flex()
                .flex_col()
                .text_size(px(BODY_FONT))
                .line_height(px(LINE_H))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(pal.text))
                .children(lines.into_iter().map(|t| div().h(px(LINE_H)).whitespace_nowrap().child(t)));
            let mut panel = div().id("bubble").absolute().left(px(x)).top(px(y)).w(px(w)).h(px(h)).child(bg).child(header).child(body);
            if footer {
                let (cur, total) = pages.unwrap_or((0, 1));
                let line_c = |on: bool| with_alpha(frame_c, if on { 0.7 } else { 0.22 });
                let btn = |id: &'static str, dir: Chevron, on: bool| {
                    div()
                        .id(id)
                        .w(px(24.0))
                        .h(px(19.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .border_1()
                        .border_color(line_c(on))
                        .child(chevron(dir, with_alpha(pal.text, if on { 0.95 } else { 0.3 })))
                };
                let mut row = div().absolute().bottom(px(PAD_B)).right(px(PAD_R - 4.0)).h(px(FOOTER_H)).flex().items_center().gap(px(4.0));
                if total > 1 {
                    row = row
                        .child(btn("prev", Chevron::Left, cur > 0).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _: &MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                this.page(-1, Instant::now());
                                cx.notify();
                                this.schedule(cx);
                            }),
                        ))
                        .child(
                            div()
                                .min_w(px(40.0))
                                .flex()
                                .justify_center()
                                .text_size(px(10.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(pal.text))
                                .child(format!("{} / {}", cur + 1, total)),
                        )
                        .child(btn("next", Chevron::Right, cur + 1 < total).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _: &MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                this.page(1, Instant::now());
                                cx.notify();
                                this.schedule(cx);
                            }),
                        ))
                        .child(div().w(px(4.0)));
                }
                row = row.child(btn("collapse", Chevron::Down, true).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        if let Some(b) = &mut this.bubble {
                            b.collapsed = true;
                        }
                        cx.notify();
                        this.schedule(cx);
                    }),
                ));
                panel = panel.child(row);
            }
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
                            eprintln!("focus-probe: SetInputFocus err={:?}; X focus now 0x{fw:x} (ours 0x{:x})", r.err(), this.x11.win);
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
                                "enter" => t.push(' '),
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
            root = root.child(panel);
        }

        // Context menu.
        if let Some((at, _)) = self.menu.filter(|_| hud) {
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
                .bg(rgb(pal.surface))
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
                        this.hide(Instant::now());
                        cx.notify();
                        this.schedule(cx);
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

        // Last: hearts, then the X input/bounding shape for exactly what this frame draws.
        let (ax, ay) = (l.avatar_x as i16, l.avatar_y as i16);
        let x11 = self.x11.clone();
        let last = self.last_shape.clone();
        let first = self.first_frame.clone();
        let t_start = self.t_start;
        let ground_px = (l.ground_x as i16, (l.ground_y - 1) as i16, 1u16, 1u16);
        let hearts = self.hearts.filter(|_| hud).map(|(t0, at)| (now.saturating_duration_since(t0).as_secs_f32() * 1000.0, at));
        let worigin = self.origin;
        let tag_clickable = collapsed;
        let tag_w = self.tag_w.clone();
        let tag_box = self.tag_box.clone();
        root.child(
            canvas(
                |_, _, _| (),
                move |_, _, _, _| {
                    let mut input: Vec<ShapeRect> =
                        if hidden { Vec::new() } else { rects.iter().map(|r| (r.0 + ax, r.1 + ay, r.2, r.3)).collect() };
                    let mut visible: Vec<ShapeRect> = body_shape.iter().map(|r| (r.0 + ax, r.1 + ay, r.2, r.3)).collect();
                    let dev = |pts: &[(f32, f32)]| pts.iter().map(|&(x, y)| (x * sf, y * sf)).collect::<Vec<_>>();
                    if let Some(g) = panel_geom {
                        input.push(to_device(Bounds::new(point(px(g.x), px(g.y)), size(px(g.w), px(g.h))), sf));
                        shape::poly_rows(&dev(&g.poly()), &mut visible);
                    }
                    if let Some(b) = menu_cell.get() {
                        input.push(to_device(b, sf));
                        visible.push(to_device(b, sf));
                    }
                    if let Some(b) = tag_cell.get() {
                        tag_w.set(f32::from(b.size.width));
                        tag_box.set(Some(b));
                        if torn {
                            visible.extend(tag_strips.iter().copied());
                        } else {
                            visible.push(to_device(b, sf));
                        }
                        if tag_clickable {
                            input.push(to_device(b, sf));
                        }
                    }
                    // Hearts are solid-colour shaped X windows, like the teleport bolt, not
                    // pixels in this window. This ARGB window is implicitly redirected (its
                    // depth differs from the root's), and presents only reach the backing
                    // pixmap inside the bounding shape. So wherever the shape grows, stale
                    // pixmap content shows (dark blobs on rising hearts) until the next present
                    // lands. The server paints a window's background together with its shape.
                    let heart_win = |layer: usize, over: Option<usize>, rgb: u32, pts: &[(f32, f32)]| {
                        let root: Vec<(f32, f32)> =
                            pts.iter().map(|&(x, y)| (x * sf + worigin.0 as f32, y * sf + worigin.1 as f32)).collect();
                        let x0 = root.iter().map(|p| p.0).fold(f32::MAX, f32::min).floor() as i32;
                        let y0 = root.iter().map(|p| p.1).fold(f32::MAX, f32::min).floor() as i32;
                        let x1 = root.iter().map(|p| p.0).fold(f32::MIN, f32::max).ceil() as i32;
                        let y1 = root.iter().map(|p| p.1).fold(f32::MIN, f32::max).ceil() as i32;
                        let local: Vec<(f32, f32)> = root.iter().map(|&(x, y)| (x - x0 as f32, y - y0 as f32)).collect();
                        let mut r = Vec::new();
                        shape::poly_rows(&local, &mut r);
                        match over {
                            Some(j) => x11.fx_set_below(layer, j, rgb, (x0, y0), (x1 - x0, y1 - y0), &r).ok(),
                            None => x11.fx_set(layer, rgb, (x0, y0), (x1 - x0, y1 - y0), &r).ok(),
                        };
                    };
                    // Rise from the click to clear air above the hair.
                    let target = (top_l - 22.0).max(16.0);
                    for (i, (dx, size, delay, colour)) in HEARTS.iter().enumerate() {
                        let (rim, fill) = (HEART_LAYER + 2 * i, HEART_LAYER + 2 * i + 1);
                        let geom = hearts.and_then(|(t, at)| {
                            let (ox, oy) = (f32::from(at.x), f32::from(at.y));
                            let p = ((t - delay) / 950.0).clamp(0.0, 1.0);
                            if p <= 0.0 || p >= 1.0 {
                                return None;
                            }
                            let ease = 1.0 - (1.0 - p).powi(3);
                            let x = ox + dx * ease;
                            let y = oy + (target - i as f32 * 16.0 - oy) * ease;
                            let scale = if p < 0.15 {
                                0.3 + 0.8 * p / 0.15
                            } else if p < 0.72 {
                                1.1 - 0.1 * (p - 0.15) / 0.57
                            } else {
                                1.0 - (p - 0.72) / 0.28
                            };
                            (scale >= 0.1).then_some((x, y, size * scale))
                        });
                        match geom {
                            Some((x, y, s)) => {
                                heart_win(fill, None, *colour, &shape::heart(x, y, s));
                                heart_win(rim, Some(fill), 0xffffff, &shape::heart(x, y, s + 5.0));
                            }
                            None => {
                                x11.fx_unmap(rim);
                                x11.fx_unmap(fill);
                            }
                        }
                    }
                    x11.flush();
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

/// Index of the pre-rotated note variant nearest `deg`.
fn frames_angle(deg: f32) -> usize {
    crate::frames::NOTE_ANGLES
        .iter()
        .enumerate()
        .min_by(|a, b| (a.1 - deg).abs().total_cmp(&(b.1 - deg).abs()))
        .map_or(3, |p| p.0)
}

/// `a` → `b` by `k` (0..1), per channel.
fn mix_rgb(a: u32, b: u32, k: f32) -> u32 {
    let ch = |sh: u32| {
        let (x, y) = (((a >> sh) & 0xff) as f32, ((b >> sh) & 0xff) as f32);
        ((x + (y - x) * k).round() as u32 & 0xff) << sh
    };
    ch(16) | ch(8) | ch(0)
}
