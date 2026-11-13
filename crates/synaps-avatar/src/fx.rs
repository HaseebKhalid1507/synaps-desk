//! Effects as pure functions of time: glitch bursts (RGB split, slice displacement,
//! scanlines), the pat squash, the breathing bob, the attention hop, hair glints and the
//! teleport bolt's geometry. The view samples them while they run and redraws only then.
//! Behaviour follows CapriCore's signal-fx / teleport choreography; the code is ours.

use crate::anim::Rng;
use crate::shape::{self, ShapeRect};
use std::f32::consts::PI;
use std::time::{Duration, Instant};

pub const CYAN: u32 = 0x00ffdd;
pub const PINK: u32 = 0xff1769;
pub const VIOLET: u32 = 0xa668ff;
pub const YELLOW: u32 = 0xedff43;
pub const BOLT_RED: u32 = 0xff003d;

fn ms(since: Duration) -> f32 {
    since.as_secs_f32() * 1000.0
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GlitchKind {
    /// Minor mood change / periodic interference.
    Burst,
    /// A big mood change (into or out of an attention state, waking up).
    Big,
    /// Coming online: bands fill in through a long stutter.
    Appear,
    /// Teleport landing: a red silhouette reassembles, then a short settle.
    Land,
    /// Dematerialise: bands drop out while the interference rises.
    Vanish,
}

/// Stutter envelopes: `(start, strength)` steps over 0..1; the last step fades out.
/// On-off-on hits like a bad signal, not one block.
const STUTTER_MINOR: [(f32, f32); 5] = [(0.0, 0.9), (0.2, 0.15), (0.34, 0.8), (0.55, 0.0), (0.66, 0.45)];
const STUTTER_BIG: [(f32, f32); 10] =
    [(0.0, 1.0), (0.11, 0.12), (0.19, 0.95), (0.33, 0.0), (0.41, 0.85), (0.5, 0.25), (0.58, 0.72), (0.7, 0.0), (0.78, 0.5), (0.86, 0.3)];
const STUTTER_APPEAR: [(f32, f32); 9] =
    [(0.0, 1.0), (0.13, 0.3), (0.21, 1.0), (0.36, 0.1), (0.45, 0.88), (0.58, 0.0), (0.66, 0.62), (0.78, 0.15), (0.85, 0.38)];

fn stutter(table: &[(f32, f32)], u: f32, seed: u64) -> f32 {
    // Steps move ±0.025 and levels ±12 % per glitch, so no two bursts match.
    let mut rng = Rng::new(seed ^ 0x5157_7e7e);
    let mut level = 0.0;
    for (i, &(at, s)) in table.iter().enumerate() {
        let at = if i == 0 { 0.0 } else { at + rng.range(-0.025, 0.025) };
        let s = (s * rng.range(0.88, 1.12)).min(1.0);
        if u < at {
            break;
        }
        level = s;
    }
    let last = table[table.len() - 1].0;
    if u >= last {
        level *= ((1.0 - u) / (1.0 - last)).clamp(0.0, 1.0);
    }
    level
}

#[derive(Clone, Copy, Debug)]
pub struct Glitch {
    pub t0: Instant,
    pub kind: GlitchKind,
    pub ms: f32,
    seed: u64,
}

impl Glitch {
    pub fn new(kind: GlitchKind, now: Instant, seed: u64) -> Glitch {
        let ms = match kind {
            GlitchKind::Burst => 440.0,
            GlitchKind::Big => 900.0,
            GlitchKind::Appear => 950.0,
            GlitchKind::Land => 330.0,
            GlitchKind::Vanish => 300.0,
        };
        Glitch { t0: now, kind, ms, seed }
    }

    pub fn done(&self, now: Instant) -> bool {
        ms(now.saturating_duration_since(self.t0)) >= self.ms
    }

    fn u(&self, now: Instant) -> f32 {
        (ms(now.saturating_duration_since(self.t0)) / self.ms).clamp(0.0, 1.0)
    }

    /// Interference 0..1.
    pub fn strength(&self, now: Instant) -> f32 {
        let u = self.u(now);
        match self.kind {
            GlitchKind::Burst => stutter(&STUTTER_MINOR, u, self.seed),
            GlitchKind::Big => stutter(&STUTTER_BIG, u, self.seed),
            GlitchKind::Appear => stutter(&STUTTER_APPEAR, u, self.seed),
            // Reassembled by ~45 %, then a short settle.
            GlitchKind::Land => {
                if u < 0.45 {
                    1.0
                } else {
                    0.6 * (1.0 - (u - 0.45) / 0.55)
                }
            }
            GlitchKind::Vanish => {
                if u < 0.15 {
                    0.6
                } else {
                    1.0
                }
            }
        }
    }

    /// Fraction of horizontal bands drawn.
    pub fn coverage(&self, now: Instant) -> f32 {
        let u = self.u(now);
        match self.kind {
            GlitchKind::Burst | GlitchKind::Big => 1.0,
            GlitchKind::Appear => 0.2 + 0.8 * (u / 0.5).min(1.0),
            // Starts a quarter in, so the body never blinks out between bolt and arrival.
            GlitchKind::Land => 0.25 + 0.75 * (u / 0.4).min(1.0),
            GlitchKind::Vanish => 1.0 - 0.8 * u,
        }
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// The face stays readable in every frame, except while dematerialising.
    pub fn keeps_face(&self) -> bool {
        self.kind != GlitchKind::Vanish
    }

    /// Teleport legs turn the figure into a red silhouette: late in the dissolve, early
    /// in the landing.
    pub fn tint(&self, now: Instant) -> Option<u32> {
        let u = self.u(now);
        match self.kind {
            GlitchKind::Vanish if u > 0.45 => Some(BOLT_RED),
            GlitchKind::Land if u < 0.3 => Some(BOLT_RED),
            _ => None,
        }
    }

    /// The pattern re-rolls every 50 ms, like a bad signal.
    pub fn step_seed(&self, now: Instant) -> u64 {
        self.seed ^ ((ms(now.saturating_duration_since(self.t0)) / 50.0) as u64).wrapping_mul(0x9e37_79b9)
    }
}

/// One glitch step's inputs. Device px, avatar-local.
#[derive(Clone, Copy, Debug)]
pub struct GlitchStep {
    pub strength: f32,
    pub coverage: f32,
    pub seed: u64,
    /// Fixes which bands appear first (per glitch).
    pub order: u64,
    /// Rows to band: from `y0` (negative reaches the HUD above the avatar rect) to `h`.
    pub y0: i32,
    pub h: i32,
    /// The frame's visible box (x, y, w, h).
    pub body: (i32, i32, i32, i32),
    /// Face rows: small offsets only, never dropped, lighter scanlines.
    pub face: Option<(i32, i32)>,
    /// Device px per logical px (band heights, split widths).
    pub sf: f32,
}

/// How the body is drawn this frame. Device px, relative to the avatar rect.
#[derive(Clone, Debug)]
pub struct BodyPlan {
    /// Squash about `pivot`.
    pub sx: f32,
    pub sy: f32,
    pub pivot: (f32, f32),
    /// Horizontal bands `(y0, y1, dx)` that are drawn; `None` = the whole body, unshifted.
    pub slices: Option<Vec<(i32, i32, i32)>>,
    /// Solid colour silhouettes under the body (RGB split), `(dx, dy, rgb)`.
    pub ghosts: Vec<(i32, i32, u32)>,
    /// Corruption cells and chroma bars, drawn over the body.
    pub cells: Vec<(ShapeRect, u32)>,
    /// Scanline darkness 0..1 (half in the face rows).
    pub scan: f32,
    pub face: Option<(i32, i32)>,
    /// Draw the body as a solid silhouette of this colour (teleport dissolve/land).
    pub tint: Option<u32>,
}

pub const SPLIT_RED: u32 = 0xff2448;

impl BodyPlan {
    pub fn plain() -> BodyPlan {
        BodyPlan {
            sx: 1.0,
            sy: 1.0,
            pivot: (0.0, 0.0),
            slices: None,
            ghosts: Vec::new(),
            cells: Vec::new(),
            scan: 0.0,
            face: None,
            tint: None,
        }
    }

    pub fn is_plain(&self) -> bool {
        self.sx == 1.0 && self.sy == 1.0 && self.slices.is_none() && self.ghosts.is_empty() && self.cells.is_empty()
    }

    /// Glitch layout for one step: bands 8–20 px tall, displaced up to ~22 % of the
    /// figure's width (a few px in the face rows), a 6–10 px red/cyan split, scanlines,
    /// corruption cells and, on the strongest steps, chroma bars across the torso.
    pub fn glitch(&mut self, g: GlitchStep) {
        let mut rng = Rng::new(g.seed | 1);
        let s = g.strength;
        let (bx, by, bw, bh) = g.body;
        let max_shift = 0.22 * bw as f32;
        let face_shift = 3.0 * g.sf;
        let in_face = |y: i32| g.face.is_some_and(|(f0, f1)| y >= f0 && y < f1);
        let mut slices = Vec::new();
        let mut y = g.y0;
        while y < g.h {
            let mut y1 = (y + ((8.0 + rng.range(0.0, 1.0).powf(1.5) * 12.0) * g.sf) as i32).min(g.h);
            // Bands never straddle the face's edges.
            if let Some((f0, f1)) = g.face {
                if y < f0 && y1 > f0 {
                    y1 = f0;
                } else if y >= f0 && y < f1 {
                    y1 = y1.min(f1);
                }
            }
            let face = in_face(y);
            let roll = rng.range(0.0, 1.0);
            let mag = rng.range(0.35, 1.0);
            let sign = if rng.range(0.0, 1.0) < 0.5 { -1.0 } else { 1.0 };
            let shift = if face {
                if s > 0.3 && roll < 0.6 { sign * face_shift * mag } else { 0.0 }
            } else if roll < 0.12 + 0.48 * s {
                sign * max_shift * mag * s
            } else {
                0.0
            };
            // Coverage: bands come and go in a stable order per glitch, not per step.
            // Band height re-rolls each step, so key the order on position, not index.
            let keep = g.coverage >= 1.0 || face || hash01(y.div_euclid(12) as u32, g.order as u32) < g.coverage;
            if keep {
                slices.push((y, y1, shift.round() as i32));
            }
            y = y1;
        }
        self.slices = Some(slices);
        self.face = g.face;
        if s > 0.15 {
            let d = ((5.0 + 5.0 * s) * g.sf / 1.08).round() as i32;
            self.ghosts.push((-d, 0, CYAN));
            self.ghosts.push((d, (rng.range(-1.0, 1.0) * 2.0) as i32, SPLIT_RED));
        }
        let n = (s * 22.0) as usize;
        for i in 0..n {
            let cw = rng.range(4.0, 18.0) as u16;
            let ch = rng.range(1.0, 4.0) as u16;
            let x = bx + (rng.range(-0.1, 1.1) * bw as f32) as i32;
            let y = by + (rng.range(0.0, 1.0) * bh as f32) as i32;
            if in_face(y) || in_face(y + ch as i32) {
                continue;
            }
            let colour = [CYAN, PINK, VIOLET, YELLOW][i % 4];
            self.cells.push(((x as i16, y as i16, cw, ch), colour));
        }
        if s > 0.7 {
            // Chroma bars across the torso (below the face, above the hips).
            let t0 = g.face.map_or(by + bh / 4, |f| f.1 + (6.0 * g.sf) as i32);
            let t1 = (by + bh * 3 / 5).max(t0 + 1);
            for (k, colour) in [YELLOW, VIOLET].into_iter().enumerate() {
                if k == 1 && rng.range(0.0, 1.0) < 0.4 {
                    break;
                }
                let y = rng.range(t0 as f32, t1 as f32) as i32;
                let hh = (rng.range(3.0, 6.0) * g.sf) as u16;
                let x0 = bx + (rng.range(-0.15, 0.35) * bw as f32) as i32;
                let w = (rng.range(0.45, 0.95) * bw as f32) as u16;
                self.cells.push(((x0 as i16, y as i16, w, hh), colour));
            }
        }
        self.scan = 0.55 * s;
    }

    /// Squash/stretch a rect about the pivot without opening gaps between bands.
    fn scale_rect(&self, r: ShapeRect) -> ShapeRect {
        if self.sx == 1.0 && self.sy == 1.0 {
            return r;
        }
        let (px, py) = self.pivot;
        let tx = |x: f32| (px + (x - px) * self.sx).round() as i32;
        let ty = |y: f32| (py + (y - py) * self.sy).round() as i32;
        let (x0, x1) = (tx(r.0 as f32), tx(r.0 as f32 + r.2 as f32));
        let (y0, y1) = (ty(r.1 as f32), ty(r.1 as f32 + r.3 as f32));
        (x0 as i16, y0 as i16, (x1 - x0).max(1) as u16, (y1 - y0).max(0) as u16)
    }

    /// The image rect after the squash: (x, y, w, h), device px.
    pub fn image_rect(&self, w: i32, h: i32) -> (f32, f32, f32, f32) {
        let (px, py) = self.pivot;
        let x0 = px + (0.0 - px) * self.sx;
        let y0 = py + (0.0 - py) * self.sy;
        (x0, y0, w as f32 * self.sx, h as f32 * self.sy)
    }

    /// The squashed body rects (no slicing).
    pub fn scaled(&self, rects: &[ShapeRect]) -> Vec<ShapeRect> {
        rects.iter().map(|&r| self.scale_rect(r)).filter(|r| r.3 > 0).collect()
    }

    /// `scaled` cut to the drawn bands and shifted with them (plus `dx`, `dy`).
    fn banded(&self, scaled: &[ShapeRect], dx: i32, dy: i32, out: &mut Vec<ShapeRect>) {
        let mv = |x: i16, y: i32, w: u16, h: i32, sx: i32| ((x as i32 + sx + dx) as i16, (y + dy) as i16, w, h as u16);
        match &self.slices {
            None => out.extend(scaled.iter().map(|&(x, y, w, h)| mv(x, y as i32, w, h as i32, 0))),
            Some(bands) => {
                for &(x, y, w, h) in scaled {
                    let (ry0, ry1) = (y as i32, y as i32 + h as i32);
                    for &(b0, b1, sx) in bands {
                        let (y0, y1) = (ry0.max(b0), ry1.min(b1));
                        if y1 > y0 {
                            out.push(mv(x, y0, w, y1 - y0, sx));
                        }
                    }
                }
            }
        }
    }

    /// The body as drawn (squashed, banded, shifted), no ghosts or cells.
    pub fn body_rects(&self, rects: &[ShapeRect]) -> Vec<ShapeRect> {
        let mut out = Vec::new();
        self.banded(&self.scaled(rects), 0, 0, &mut out);
        out
    }

    /// The RGB-split silhouettes, following the drawn bands: `(rect, rgb)`.
    pub fn ghost_rects(&self, rects: &[ShapeRect]) -> Vec<(ShapeRect, u32)> {
        let scaled = self.scaled(rects);
        let mut out = Vec::new();
        let mut tmp = Vec::new();
        for &(dx, dy, c) in &self.ghosts {
            tmp.clear();
            self.banded(&scaled, dx, dy, &mut tmp);
            out.extend(tmp.iter().map(|&r| (r, c)));
        }
        out
    }

    /// Everything this plan draws, as shape rects (avatar-local).
    pub fn shape(&self, rects: &[ShapeRect]) -> Vec<ShapeRect> {
        let scaled = self.scaled(rects);
        let mut out = Vec::with_capacity(scaled.len() * (1 + self.ghosts.len()));
        self.banded(&scaled, 0, 0, &mut out);
        for &(dx, dy, _) in &self.ghosts {
            self.banded(&scaled, dx, dy, &mut out);
        }
        out.extend(self.cells.iter().map(|c| c.0));
        out
    }
}

fn hash01(a: u32, b: u32) -> f32 {
    let mut x = a.wrapping_mul(0x9e37_79b1) ^ b.wrapping_mul(0x85eb_ca6b);
    x ^= x >> 15;
    x = x.wrapping_mul(0x2c1b_3c6d);
    x ^= x >> 12;
    (x >> 8) as f32 / (1u32 << 24) as f32
}

/// The pat squash: a quick 7 % vertical squash, then a damped spring back.
pub const SQUASH_MS: f32 = 560.0;
pub fn squash(age: Duration) -> Option<(f32, f32)> {
    let t = ms(age);
    if t >= SQUASH_MS {
        return None;
    }
    let sy = if t < 80.0 {
        let u = t / 80.0;
        1.0 - 0.07 * (1.0 - (1.0 - u) * (1.0 - u))
    } else {
        let t = t - 80.0;
        1.0 - 0.07 * (-t / 120.0).exp() * (2.0 * PI * t / 280.0).cos()
    };
    // Never past the rest silhouette (no stretch, no widening): without a compositor the
    // shape may only grow back into pixels the view has painted (see the body canvas).
    Some((1.0, sy.min(1.0)))
}

/// Breathing bob, device px upward: 0..=px on a cosine.
pub fn bob(elapsed: Duration, px: i32, period_ms: u64) -> i32 {
    if px <= 0 || period_ms == 0 {
        return 0;
    }
    let ph = ms(elapsed) / period_ms as f32 * 2.0 * PI;
    (px as f32 * (0.5 - 0.5 * ph.cos())).round() as i32
}

/// When the bob's rounded value next changes (searched in 20 ms steps).
pub fn bob_next_change(t0: Instant, now: Instant, px: i32, period_ms: u64) -> Option<Instant> {
    if px <= 0 || period_ms == 0 {
        return None;
    }
    let cur = bob(now.saturating_duration_since(t0), px, period_ms);
    let step = Duration::from_millis(20);
    let mut t = now;
    for _ in 0..(period_ms / 20 + 2) {
        t += step;
        if bob(t.saturating_duration_since(t0), px, period_ms) != cur {
            return Some(t);
        }
    }
    Some(now + Duration::from_millis(period_ms))
}

/// The attention hop: up and down once, device px upward.
pub const HOP_MS: f32 = 440.0;
pub fn hop(age: Duration, height: f32) -> Option<i32> {
    let t = ms(age);
    if t >= HOP_MS {
        return None;
    }
    let u = t / HOP_MS;
    // Two beats: a full hop, then a little one.
    let y = if u < 0.68 {
        let v = u / 0.68;
        4.0 * v * (1.0 - v)
    } else {
        let v = (u - 0.68) / 0.32;
        0.3 * 4.0 * v * (1.0 - v)
    };
    Some((y * height).round() as i32)
}

/// A glint on the hair: grows and shrinks once.
#[derive(Clone, Copy, Debug)]
pub struct Glint {
    pub t0: Instant,
    pub x: f32,
    pub y: f32,
    pub size: f32,
    pub spin: f32,
}

impl Glint {
    /// (scale 0..1) or None when finished.
    pub fn scale(&self, now: Instant, life_ms: u64) -> Option<f32> {
        let u = ms(now.saturating_duration_since(self.t0)) / life_ms as f32;
        if u >= 1.0 {
            return None;
        }
        Some((PI * u).sin().powf(1.6))
    }
}

/// Four-point star, `r` arm length.
pub fn star(cx: f32, cy: f32, r: f32, spin: f32) -> Vec<(f32, f32)> {
    (0..8)
        .map(|i| {
            let a = spin + i as f32 * PI / 4.0;
            let rr = if i % 2 == 0 { r } else { r * 0.2 };
            (cx + a.cos() * rr, cy + a.sin() * rr)
        })
        .collect()
}

// ---- teleport bolt ------------------------------------------------------------------

pub const BOLT_MS: f32 = 420.0;
/// The body lands (window moves, still hidden) at this point of the bolt…
pub const BOLT_ARRIVE: f32 = 0.5;
/// …and the landing glitch takes over here, as the tail reaches the target.
pub const BOLT_END: f32 = 0.88;
/// Bolt layer colours, bottom to top: outer halo, halo, bolt, core.
pub const BOLT_COLOURS: [u32; 4] = [0x560a1b, 0xa3112e, BOLT_RED, 0xffeef3];

#[derive(Clone, Copy, Debug)]
struct RoutePt {
    x: f32,
    y: f32,
    t: f32,
}

/// A jagged route from `a` to `b` (root px): alternating elbows across the axis.
pub struct Bolt {
    route: Vec<RoutePt>,
    seed: u64,
    scale: f32,
}

fn along(p: (f32, f32), axis: (f32, f32), fwd: f32, across: f32) -> (f32, f32) {
    (p.0 + axis.0 * fwd - axis.1 * across, p.1 + axis.1 * fwd + axis.0 * across)
}

impl Bolt {
    /// `scale`: figure size in px (bolt thickness and branch reach follow it).
    pub fn new(a: (f32, f32), b: (f32, f32), seed: u64, scale: f32) -> Bolt {
        let mut rng = Rng::new(seed | 1);
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len = (dx * dx + dy * dy).sqrt().max(1.0);
        let axis = (dx / len, dy / len);
        let amp = (len * 0.11).clamp(16.0, 90.0);
        let mirror = if rng.range(0.0, 1.0) < 0.5 { -1.0 } else { 1.0 };
        let mut route = vec![RoutePt { x: a.0, y: a.1, t: 0.0 }];
        for &(base, off) in &[(0.13, -0.3), (0.27, 0.66), (0.41, -0.6), (0.55, 0.78), (0.69, -0.26), (0.8, 0.7), (0.91, -0.16)] {
            let t = base + rng.range(-0.02, 0.02);
            let bend = off * amp * mirror * rng.range(0.73, 1.18);
            let prev = *route.last().expect("route start");
            let corner = along(a, axis, t * len, bend);
            // A small opposing crack on the way into each elbow.
            let crack = rng.range(0.18, 0.34) * amp * 0.6;
            let mid = ((prev.x + (corner.0 - prev.x) * 0.62), (prev.y + (corner.1 - prev.y) * 0.62));
            let mid = along(mid, axis, 0.0, -bend.signum() * crack);
            route.push(RoutePt { x: mid.0, y: mid.1, t: prev.t + (t - prev.t) * 0.62 });
            route.push(RoutePt { x: corner.0, y: corner.1, t });
        }
        route.push(RoutePt { x: b.0, y: b.1, t: 1.0 });
        Bolt { route, seed, scale }
    }

    fn at(&self, t: f32) -> (f32, f32) {
        let t = t.clamp(0.0, 1.0);
        let i = self.route.iter().position(|p| p.t >= t).unwrap_or(self.route.len() - 1).max(1);
        let (a, b) = (self.route[i - 1], self.route[i]);
        let u = if b.t > a.t { (t - a.t) / (b.t - a.t) } else { 0.0 };
        (a.x + (b.x - a.x) * u, a.y + (b.y - a.y) * u)
    }

    pub fn bbox(&self) -> (i32, i32, i32, i32) {
        let pad = self.scale * 0.5 + 24.0;
        let x0 = self.route.iter().map(|p| p.x).fold(f32::MAX, f32::min) - pad;
        let y0 = self.route.iter().map(|p| p.y).fold(f32::MAX, f32::min) - pad;
        let x1 = self.route.iter().map(|p| p.x).fold(f32::MIN, f32::max) + pad;
        let y1 = self.route.iter().map(|p| p.y).fold(f32::MIN, f32::max) + pad;
        (x0 as i32, y0 as i32, (x1 - x0) as i32, (y1 - y0) as i32)
    }

    /// Head and tail positions along the route at time `u` (0..1 of BOLT_MS): the head
    /// strikes out from the origin, the full bolt holds, then the tail retracts into the
    /// target.
    pub fn span(u: f32) -> (f32, f32) {
        let ease = |v: f32| 1.0 - (1.0 - v.clamp(0.0, 1.0)).powi(3);
        (ease(0.04 + u / 0.28), ease((u - 0.6) / 0.3))
    }

    /// Width multiplier: a flash when the head strikes, then a 35 ms flicker.
    fn pulse(u: f32) -> f32 {
        if (0.27..0.38).contains(&u) {
            return 1.3;
        }
        if ((u * BOLT_MS / 35.0) as i32) % 2 == 1 { 0.8 } else { 1.0 }
    }

    /// Ribbon polygons per layer, root px: `[outer halo, halo, bolt, core]`. No alpha
    /// without a compositor, so the glow is stacked, progressively wider dark-red shapes.
    pub fn frame(&self, u: f32) -> [Vec<Vec<(f32, f32)>>; 4] {
        let mut out: [Vec<Vec<(f32, f32)>>; 4] = Default::default();
        let (head, tail) = Bolt::span(u);
        if head <= tail + 0.004 {
            return out;
        }
        let travelled = head - tail;
        let mut seg: Vec<(f32, f32, f32)> = vec![{
            let p = self.at(tail);
            (p.0, p.1, tail)
        }];
        seg.extend(self.route.iter().filter(|p| p.t > tail && p.t < head).map(|p| (p.x, p.y, p.t)));
        let hp = self.at(head);
        seg.push((hp.0, hp.1, head));
        let radius = (self.scale * 0.1).clamp(6.0, 28.0) * Bolt::pulse(u);
        let n = seg.len();
        let widths: Vec<f32> = seg
            .iter()
            .enumerate()
            .map(|(i, p)| {
                if i == 0 || i == n - 1 {
                    0.6
                } else {
                    let f = (p.2 - tail) / travelled;
                    radius * (0.5 + 0.55 * f + if i % 2 == 1 { 0.18 } else { -0.15 })
                }
            })
            .collect();
        let pts: Vec<(f32, f32)> = seg.iter().map(|p| (p.0, p.1)).collect();
        let grow = |k: f32, add: f32| widths.iter().map(|w| w * k + add).collect::<Vec<_>>();
        out[0].push(ribbon(&pts, &grow(2.3, 7.0)));
        out[1].push(ribbon(&pts, &grow(1.6, 3.0)));
        out[2].push(ribbon(&pts, &widths));
        out[3].push(ribbon(&pts, &grow(0.34, 0.0)));
        // Two forks off the travelled part.
        if travelled > 0.08 {
            let mut rng = Rng::new(self.seed ^ 0xf27a_64b5);
            for (k, base) in [0.36f32, 0.68].into_iter().enumerate() {
                let t = base + rng.range(-0.06, 0.06);
                let side = if k % 2 == 1 { 1.0 } else { -1.0 } * if self.seed & 1 == 0 { 1.0 } else { -1.0 };
                let var = rng.range(0.85, 1.2);
                if t <= tail || t >= head {
                    continue;
                }
                let o = self.at(t);
                let (p0, p1) = (self.at(t - 0.05), self.at(t + 0.05));
                let (ax, ay) = (p1.0 - p0.0, p1.1 - p0.1);
                let al = (ax * ax + ay * ay).sqrt().max(1e-3);
                let axis = (ax / al, ay / al);
                let growth = ((t - tail) / 0.07).min((head - t) / 0.08).min(1.0);
                let reach = self.scale * 0.32 * growth * var;
                if reach < 3.0 {
                    continue;
                }
                let fork = |f: f32, a: f32| along(o, axis, reach * f, side * reach * a);
                let stem = [o, fork(-0.16, 0.22), fork(-0.12, 0.4), fork(-0.49, 0.48), fork(-0.39, 0.68), fork(-0.86, 0.88), fork(-1.15, 1.04)];
                let th = growth * (self.scale * 0.016).clamp(2.0, 4.5);
                let w: Vec<f32> = [1.45, 2.05, 1.65, 1.2, 0.8, 0.55, 0.12].iter().map(|v| v * th).collect();
                out[1].push(ribbon(&stem, &w.iter().map(|v| v * 1.7 + 2.0).collect::<Vec<_>>()));
                out[2].push(ribbon(&stem, &w));
            }
        }
        out
    }
}

/// Asymmetric ribbon around a polyline: a filled polygon.
fn ribbon(pts: &[(f32, f32)], widths: &[f32]) -> Vec<(f32, f32)> {
    let n = pts.len();
    let mut upper = Vec::with_capacity(n);
    let mut lower = Vec::with_capacity(n);
    for i in 0..n {
        let (l, r) = (pts[i.saturating_sub(1)], pts[(i + 1).min(n - 1)]);
        let (ax, ay) = (r.0 - l.0, r.1 - l.1);
        let al = (ax * ax + ay * ay).sqrt().max(1e-3);
        let axis = (ax / al, ay / al);
        let w = widths[i];
        upper.push(along(pts[i], axis, 0.0, w * if i % 2 == 1 { 0.63 } else { 1.12 }));
        lower.push(along(pts[i], axis, 0.0, -w * if i % 2 == 1 { 1.06 } else { 0.59 }));
    }
    upper.extend(lower.into_iter().rev());
    upper
}

/// Polygons (root px) → shape rects relative to `origin`.
/// Polygons → shape rows, edges snapped to a `step` px grid (Capri's bolt is blocky).
pub fn polys_to_rects(polys: &[Vec<(f32, f32)>], origin: (i32, i32), step: i32) -> Vec<ShapeRect> {
    let mut rows = Vec::new();
    for p in polys {
        let local: Vec<(f32, f32)> = p.iter().map(|&(x, y)| (x - origin.0 as f32, y - origin.1 as f32)).collect();
        shape::poly_rows(&local, &mut rows);
    }
    if step <= 1 {
        return rows;
    }
    // Snap each run to the grid, per band of `step / 2` rows, so the edge stair-steps.
    let band = (step / 2).max(1);
    rows.into_iter()
        .filter_map(|(x, y, w, h)| {
            let b = (y as i32).div_euclid(band) as u32;
            let jitter = (hash01(b, x as u32 / step as u32) * step as f32) as i32;
            let x0 = (x as i32 + jitter / 2).div_euclid(step) * step;
            let x1 = (x as i32 + w as i32 + jitter / 2).div_euclid(step) * step;
            (x1 > x0).then_some((x0 as i16, y, (x1 - x0) as u16, h))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn squash_springs_back() {
        let (_, sy) = squash(Duration::from_millis(80)).unwrap();
        assert!((sy - 0.93).abs() < 0.01);
        assert!(squash(Duration::from_millis(600)).is_none());
    }

    #[test]
    fn bob_range() {
        let v: Vec<i32> = (0..36).map(|i| bob(Duration::from_millis(i * 100), 2, 3_600)).collect();
        assert_eq!(*v.iter().min().unwrap(), 0);
        assert_eq!(*v.iter().max().unwrap(), 2);
    }

    #[test]
    fn glitch_vanish_thins_to_red() {
        let t = Instant::now();
        let g = Glitch::new(GlitchKind::Vanish, t, 7);
        let step = |at: Instant| GlitchStep {
            strength: g.strength(at),
            coverage: g.coverage(at),
            seed: g.step_seed(at),
            order: g.seed(),
            y0: 0,
            h: 400,
            body: (0, 0, 300, 400),
            face: None,
            sf: 1.0,
        };
        let mut plan = BodyPlan::plain();
        plan.glitch(step(t));
        let full: i32 = plan.slices.unwrap().iter().map(|b| b.1 - b.0).sum();
        let late = t + Duration::from_millis(290);
        let mut plan = BodyPlan::plain();
        plan.glitch(step(late));
        let thin: i32 = plan.slices.unwrap().iter().map(|b| b.1 - b.0).sum();
        assert!(thin * 2 < full, "{thin} vs {full}");
        assert_eq!(g.tint(late), Some(BOLT_RED));
        assert_eq!(g.tint(t), None);
    }

    #[test]
    fn glitch_keeps_the_face() {
        let t = Instant::now();
        for kind in [GlitchKind::Big, GlitchKind::Appear, GlitchKind::Burst] {
            let g = Glitch::new(kind, t, 11);
            for ms in (0..950).step_by(25) {
                let at = t + Duration::from_millis(ms);
                let mut plan = BodyPlan::plain();
                plan.glitch(GlitchStep {
                    strength: g.strength(at),
                    coverage: g.coverage(at),
                    seed: g.step_seed(at),
                    order: g.seed(),
                    y0: -80,
                    h: 400,
                    body: (20, 10, 300, 390),
                    face: Some((90, 140)),
                    sf: 1.0,
                });
                let face: Vec<_> = plan.slices.unwrap().into_iter().filter(|b| b.1 > 90 && b.0 < 140).collect();
                assert_eq!(face.iter().map(|b| b.1 - b.0).sum::<i32>(), 50, "{kind:?} {ms}");
                assert!(face.iter().all(|b| b.2.abs() <= 3 && b.0 >= 90 && b.1 <= 140));
            }
        }
    }

    #[test]
    fn big_glitch_stutters() {
        let t = Instant::now();
        let g = Glitch::new(GlitchKind::Big, t, 5);
        let v: Vec<f32> = (0..90).map(|i| g.strength(t + Duration::from_millis(i * 10))).collect();
        let flips = v.windows(2).filter(|w| (w[0] > 0.5) != (w[1] > 0.5)).count();
        assert!(flips >= 4, "{flips}");
        assert!(g.ms >= 800.0);
    }

    #[test]
    fn bolt_reaches_both_ends() {
        let b = Bolt::new((0.0, 0.0), (400.0, 100.0), 3, 300.0);
        assert_eq!(b.at(0.0), (0.0, 0.0));
        assert_eq!(b.at(1.0), (400.0, 100.0));
        let layers = b.frame(0.5);
        assert!(layers.iter().all(|l| !l.is_empty()));
        // Visible (the full bolt) from the strike through the hold: 3+ frames at 30 fps.
        let full: Vec<u32> = (0..42).filter(|&i| { let (h, t) = Bolt::span(i as f32 / 42.0); h > 0.95 && t < 0.05 }).collect();
        assert!(full.len() as f32 * BOLT_MS / 42.0 >= 100.0, "{full:?}");
    }
}
