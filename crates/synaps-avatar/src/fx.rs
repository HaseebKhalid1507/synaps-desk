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
pub const BOLT_DARK: u32 = 0xb3102f;

fn ms(since: Duration) -> f32 {
    since.as_secs_f32() * 1000.0
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GlitchKind {
    /// Mood change / periodic interference.
    Burst,
    /// Materialise: bands fill in while the interference decays.
    Appear,
    /// Dematerialise: bands drop out while the interference rises.
    Vanish,
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
            GlitchKind::Burst => 380.0,
            GlitchKind::Appear => 420.0,
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

    /// Interference 0..1: Capri's stutter envelope (hit, gap, hit, gap, tail), squeezed.
    pub fn strength(&self, now: Instant) -> f32 {
        let u = self.u(now);
        match self.kind {
            GlitchKind::Burst => {
                if u < 0.12 {
                    0.96
                } else if u < 0.25 {
                    0.28
                } else if u < 0.45 {
                    0.86
                } else if u < 0.58 {
                    0.12
                } else {
                    0.5 * (1.0 - (u - 0.58) / 0.42)
                }
            }
            GlitchKind::Appear => {
                if u < 0.5 {
                    1.0
                } else if u < 0.62 {
                    0.25
                } else {
                    0.7 * (1.0 - (u - 0.62) / 0.38)
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
            GlitchKind::Burst => 1.0,
            // Starts a quarter in, so the body never blinks out between bolt and arrival.
            GlitchKind::Appear => 0.25 + 0.75 * (u / 0.55).min(1.0),
            GlitchKind::Vanish => 1.0 - 0.8 * u,
        }
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// Teleport legs turn the figure into a red silhouette: late in the dissolve, early
    /// in the landing.
    pub fn tint(&self, now: Instant) -> Option<u32> {
        let u = self.u(now);
        match self.kind {
            GlitchKind::Vanish if u > 0.45 => Some(BOLT_RED),
            GlitchKind::Appear if u < 0.3 => Some(BOLT_RED),
            _ => None,
        }
    }

    /// The pattern re-rolls every 50 ms, like a bad signal.
    pub fn step_seed(&self, now: Instant) -> u64 {
        self.seed ^ ((ms(now.saturating_duration_since(self.t0)) / 50.0) as u64).wrapping_mul(0x9e37_79b9)
    }
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
    /// Corruption cells.
    pub cells: Vec<(ShapeRect, u32)>,
    /// Scanline darkness 0..1.
    pub scan: f32,
    /// Draw the body as a solid silhouette of this colour (teleport dissolve/land).
    pub tint: Option<u32>,
}

impl BodyPlan {
    pub fn plain() -> BodyPlan {
        BodyPlan { sx: 1.0, sy: 1.0, pivot: (0.0, 0.0), slices: None, ghosts: Vec::new(), cells: Vec::new(), scan: 0.0, tint: None }
    }

    pub fn is_plain(&self) -> bool {
        self.sx == 1.0 && self.sy == 1.0 && self.slices.is_none() && self.ghosts.is_empty() && self.cells.is_empty()
    }

    /// Glitch layout for one step. `w`, `h`: the avatar rect; `body`: the frame's visible
    /// box (cells land near the figure).
    /// `seed` re-rolls the pattern; `order` fixes which bands appear first (per glitch).
    #[allow(clippy::too_many_arguments)]
    pub fn glitch(&mut self, strength: f32, coverage: f32, seed: u64, order: u64, _w: i32, h: i32, body: (i32, i32, i32, i32)) {
        let mut rng = Rng::new(seed | 1);
        let mut slices = Vec::new();
        let mut y = 0;
        while y < h {
            let bh = (4.0 + rng.range(0.0, 1.0).powi(2) * 46.0) as i32;
            let y1 = (y + bh).min(h);
            let shift = if rng.range(0.0, 1.0) < 0.2 + 0.5 * strength { rng.range(-1.0, 1.0) * 38.0 * strength } else { 0.0 };
            // Coverage: bands come and go in a stable order per glitch, not per step.
            // Band height re-rolls each step, so key the order on position, not index.
            let keep = coverage >= 1.0 || hash01((y / 12) as u32, order as u32) < coverage;
            if keep {
                slices.push((y, y1, shift.round() as i32));
            }
            y = y1;
        }
        self.slices = Some(slices);
        if strength > 0.18 {
            let d = (2.0 + 8.0 * strength).round() as i32;
            if rng.range(0.0, 1.0) < 0.85 {
                self.ghosts.push((-d, 0, CYAN));
            }
            if rng.range(0.0, 1.0) < 0.85 {
                self.ghosts.push((d, (rng.range(-1.0, 1.0) * 2.0) as i32, PINK));
            }
        }
        let n = (strength * 26.0) as usize;
        let (bx, by, bw, bh) = body;
        for i in 0..n {
            let cw = rng.range(3.0, 14.0) as u16;
            let ch = rng.range(1.0, 4.0) as u16;
            let x = bx + (rng.range(-0.1, 1.1) * bw as f32) as i32;
            let y = by + (rng.range(0.0, 1.0) * bh as f32) as i32;
            let colour = [CYAN, PINK, VIOLET, YELLOW][i % 4];
            self.cells.push(((x as i16, y as i16, cw, ch), colour));
        }
        self.scan = 0.45 * strength;
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

pub const BOLT_MS: f32 = 340.0;

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
    /// shoots out, the tail chases it in.
    pub fn span(u: f32) -> (f32, f32) {
        let ease = |v: f32| 1.0 - (1.0 - v.clamp(0.0, 1.0)).powi(3);
        (ease(0.06 + u / 0.5), ease((u - 0.32) / 0.6))
    }

    /// Ribbon polygons `(main, branches)` between tail and head, root px.
    pub fn frame(&self, u: f32) -> (Vec<Vec<(f32, f32)>>, Vec<Vec<(f32, f32)>>) {
        let (head, tail) = Bolt::span(u);
        let mut main = Vec::new();
        let mut branches = Vec::new();
        if head <= tail + 0.004 {
            return (main, branches);
        }
        let travelled = head - tail;
        let mut seg: Vec<(f32, f32, f32)> = vec![{
            let p = self.at(tail);
            (p.0, p.1, tail)
        }];
        seg.extend(self.route.iter().filter(|p| p.t > tail && p.t < head).map(|p| (p.x, p.y, p.t)));
        let hp = self.at(head);
        seg.push((hp.0, hp.1, head));
        let radius = (self.scale * 0.1).clamp(6.0, 28.0);
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
        main.push(ribbon(&seg.iter().map(|p| (p.0, p.1)).collect::<Vec<_>>(), &widths));
        // Forks off the travelled part.
        if travelled > 0.08 {
            let mut rng = Rng::new(self.seed ^ 0xf27a_64b5);
            let count = 7;
            for k in 0..count {
                let t = 0.1 + (k as f32 + 0.15 + rng.range(0.0, 0.7)) / count as f32 * 0.82;
                let side = if k % 2 == 1 { 1.0 } else { -1.0 };
                let var = rng.range(0.75, 1.25);
                if t <= tail || t >= head {
                    continue;
                }
                let o = self.at(t);
                let (p0, p1) = (self.at(t - 0.05), self.at(t + 0.05));
                let (ax, ay) = (p1.0 - p0.0, p1.1 - p0.1);
                let al = (ax * ax + ay * ay).sqrt().max(1e-3);
                let axis = (ax / al, ay / al);
                let growth = ((t - tail) / 0.07).min((head - t) / 0.08).min(1.0);
                let reach = self.scale * 0.3 * growth * var;
                if reach < 3.0 {
                    continue;
                }
                let fork = |f: f32, a: f32| along(o, axis, reach * f, side * reach * a);
                let stem = [o, fork(-0.16, 0.22), fork(-0.12, 0.4), fork(-0.49, 0.48), fork(-0.39, 0.68), fork(-0.86, 0.88), fork(-1.15, 1.04)];
                let th = growth * (self.scale * 0.012).clamp(1.6, 3.4);
                let w: Vec<f32> = [1.45, 2.05, 1.65, 1.2, 0.8, 0.55, 0.12].iter().map(|v| v * th).collect();
                branches.push(ribbon(&stem, &w));
            }
        }
        (main, branches)
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
        let mut plan = BodyPlan::plain();
        plan.glitch(g.strength(t), g.coverage(t), g.step_seed(t), g.seed(), 300, 400, (0, 0, 300, 400));
        let full: i32 = plan.slices.unwrap().iter().map(|b| b.1 - b.0).sum();
        let late = t + Duration::from_millis(290);
        let mut plan = BodyPlan::plain();
        plan.glitch(g.strength(late), g.coverage(late), g.step_seed(late), g.seed(), 300, 400, (0, 0, 300, 400));
        let thin: i32 = plan.slices.unwrap().iter().map(|b| b.1 - b.0).sum();
        assert!(thin * 2 < full, "{thin} vs {full}");
        assert_eq!(g.tint(late), Some(BOLT_RED));
        assert_eq!(g.tint(t), None);
    }

    #[test]
    fn bolt_reaches_both_ends() {
        let b = Bolt::new((0.0, 0.0), (400.0, 100.0), 3, 300.0);
        assert_eq!(b.at(0.0), (0.0, 0.0));
        assert_eq!(b.at(1.0), (400.0, 100.0));
        let (main, _) = b.frame(0.5);
        assert_eq!(main.len(), 1);
    }
}
