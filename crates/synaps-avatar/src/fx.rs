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

/// Stutter envelopes: `(start, strength)` steps over 0..1. A main hit (the pattern
/// re-rolls every 50 ms inside it), then 2–3 short stutter frames with clean gaps, then
/// settled. The last step fades out.
const STUTTER_MINOR: [(f32, f32); 6] = [(0.0, 0.85), (0.5, 0.0), (0.6, 0.6), (0.69, 0.0), (0.8, 0.45), (0.88, 0.0)];
const STUTTER_BIG: [(f32, f32); 8] = [(0.0, 1.0), (0.64, 0.0), (0.71, 0.7), (0.76, 0.0), (0.83, 0.55), (0.88, 0.0), (0.93, 0.4), (0.97, 0.0)];
const STUTTER_APPEAR: [(f32, f32); 6] = [(0.0, 1.0), (0.6, 0.0), (0.68, 0.65), (0.74, 0.0), (0.83, 0.45), (0.89, 0.0)];

fn stutter(table: &[(f32, f32)], u: f32, seed: u64) -> f32 {
    // Steps move ±0.01 and levels ±12 % per glitch, so no two bursts match.
    let mut rng = Rng::new(seed ^ 0x5157_7e7e);
    let mut level = 0.0;
    for (i, &(at, s)) in table.iter().enumerate() {
        let at = if i == 0 { 0.0 } else { at + rng.range(-0.01, 0.01) };
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
            GlitchKind::Burst => 480.0,
            GlitchKind::Big => 850.0,
            GlitchKind::Appear => 900.0,
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

    /// Fraction of horizontal bands drawn (teleport legs only; a glitch keeps them all).
    pub fn coverage(&self, now: Instant) -> f32 {
        let u = self.u(now);
        match self.kind {
            GlitchKind::Burst | GlitchKind::Big | GlitchKind::Appear => 1.0,
            // Starts a quarter in, so the body never blinks out between bolt and arrival.
            GlitchKind::Land => 0.25 + 0.75 * (u / 0.4).min(1.0),
            GlitchKind::Vanish => 1.0 - 0.8 * u,
        }
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// Fraction of bands drawn as a single colour channel instead of the image: coming
    /// online, the figure builds up from channel ghosts to full colour in ~400 ms, never
    /// from nothing.
    pub fn solid(&self, now: Instant) -> f32 {
        match self.kind {
            GlitchKind::Appear => 0.7 * (1.0 - ms(now.saturating_duration_since(self.t0)) / 400.0).max(0.0),
            _ => 0.0,
        }
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
    /// Face rows: small offsets only, never dropped, no split, lighter scanlines.
    pub face: Option<(i32, i32)>,
    /// Fraction of bands drawn as a solid colour channel (see `Glitch::solid`).
    pub solid: f32,
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
    /// Horizontal bands `(y0, y1, dx, dy)` that are drawn: source rows y0..y1, moved by
    /// (dx, dy). `None` = the whole body, unshifted.
    pub slices: Option<Vec<(i32, i32, i32, i32)>>,
    /// Per band (parallel to `slices`; empty = none): the red/blue split, device px. The
    /// band is drawn again `split` px to the left in red and to the right in blue, under it.
    pub split: Vec<i32>,
    /// Per band (parallel to `slices`; empty = none): draw the band as this solid colour.
    pub solid: Vec<Option<u32>>,
    /// Corruption cells and chroma bars, drawn over the body.
    pub cells: Vec<(ShapeRect, u32)>,
    /// Scanline darkness 0..1 (half in the face rows).
    pub scan: f32,
    pub face: Option<(i32, i32)>,
    /// Draw the body as a solid silhouette of this colour (teleport dissolve/land).
    pub tint: Option<u32>,
}

pub const SPLIT_RED: u32 = 0xff2448;
pub const SPLIT_BLUE: u32 = 0x2e7bff;

impl BodyPlan {
    pub fn plain() -> BodyPlan {
        BodyPlan {
            sx: 1.0,
            sy: 1.0,
            pivot: (0.0, 0.0),
            slices: None,
            split: Vec::new(),
            solid: Vec::new(),
            cells: Vec::new(),
            scan: 0.0,
            face: None,
            tint: None,
        }
    }

    pub fn is_plain(&self) -> bool {
        self.sx == 1.0 && self.sy == 1.0 && self.slices.is_none() && self.cells.is_empty()
    }

    pub fn has_ghosts(&self) -> bool {
        self.split.iter().any(|&d| d != 0)
    }

    /// Glitch layout for one step. The whole silhouette stays drawn: 6–12 bands of random
    /// height across the figure, each moved sideways 10–40 px, each with its own 4–8 px
    /// red/blue split; 1–2 chroma bars across the torso; faint scanlines. The face rows
    /// are one band with at most a few px of shift and no split. Rows above the figure
    /// (the status tag) tear in two more bands.
    pub fn glitch(&mut self, g: GlitchStep) {
        let mut rng = Rng::new(g.seed | 1);
        let s = g.strength;
        let amp = (s * 1.25).min(1.0);
        let (bx, by, bw, bh) = g.body;
        let top = by.clamp(g.y0, g.h);
        // Band edges: two over the HUD, 6–12 over the figure, the face's own edges.
        let mut cuts = vec![g.y0, top, g.h];
        if top - g.y0 > 8 {
            cuts.push(g.y0 + (top - g.y0) * rng.range(30.0, 70.0) as i32 / 100);
        }
        let n = 6 + rng.pick(7);
        let min_h = (10.0 * g.sf) as i32;
        for _ in 1..n {
            cuts.push(rng.range(top as f32, g.h as f32) as i32);
        }
        if let Some((f0, f1)) = g.face {
            cuts.retain(|&c| c <= f0 || c >= f1);
            cuts.push(f0.clamp(g.y0, g.h));
            cuts.push(f1.clamp(g.y0, g.h));
        }
        cuts.sort_unstable();
        cuts.dedup();
        // Merge slivers into their neighbour (never across the face's edges).
        let is_face_edge = |c: i32| g.face.is_some_and(|(f0, f1)| c == f0 || c == f1);
        let mut merged: Vec<i32> = Vec::new();
        for c in cuts {
            match merged.last() {
                Some(&p) if c - p < min_h && !is_face_edge(c) && c != g.h && c != top => {}
                _ => merged.push(c),
            }
        }
        let mut slices = Vec::new();
        let mut split = Vec::new();
        let mut solid = Vec::new();
        let mut sign = if rng.range(0.0, 1.0) < 0.5 { -1.0 } else { 1.0 };
        for (k, w) in merged.windows(2).enumerate() {
            let (y0, y1) = (w[0], w[1]);
            if y1 <= y0 {
                continue;
            }
            let face = g.face.is_some_and(|(f0, f1)| y0 >= f0 && y1 <= f1);
            // Mostly alternating directions, so neighbouring bands tear apart.
            if rng.range(0.0, 1.0) < 0.75 {
                sign = -sign;
            }
            let (dx, d) = if face {
                ((sign * rng.range(0.0, 3.0) * g.sf * amp).round() as i32, 0)
            } else if rng.range(0.0, 1.0) < 0.88 {
                ((sign * rng.range(10.0, 40.0) * g.sf * amp).round() as i32, (rng.range(4.0, 8.0) * g.sf * amp).round() as i32)
            } else {
                (0, (rng.range(4.0, 8.0) * g.sf * amp).round() as i32)
            };
            // Teleport legs drop bands (coverage); a glitch never does.
            let keep = g.coverage >= 1.0 || face || hash01(y0.div_euclid(12) as u32, g.order as u32) < g.coverage;
            if !keep {
                continue;
            }
            let channel = (!face && g.solid > 0.0 && hash01(k as u32 + 17, g.order as u32 ^ 0x5a5a) < g.solid)
                .then(|| if k % 2 == 0 { SPLIT_RED } else { SPLIT_BLUE });
            slices.push((y0, y1, dx, 0));
            split.push(d);
            solid.push(channel);
        }
        self.slices = Some(slices);
        self.split = split;
        self.solid = solid;
        self.face = g.face;
        let in_face = |y: i32| g.face.is_some_and(|(f0, f1)| y >= f0 && y < f1);
        // A few corruption cells (never on the face).
        for i in 0..(s * 8.0) as usize {
            let cw = (rng.range(6.0, 22.0) * g.sf) as u16;
            let ch = (rng.range(1.0, 3.0) * g.sf).max(1.0) as u16;
            let x = bx + (rng.range(-0.1, 1.1) * bw as f32) as i32;
            let y = by + (rng.range(0.0, 1.0) * bh as f32) as i32;
            if in_face(y) || in_face(y + ch as i32) {
                continue;
            }
            self.cells.push(((x as i16, y as i16, cw, ch), [CYAN, PINK, VIOLET, YELLOW][i % 4]));
        }
        // 1–2 solid chroma bars crossing the body (below the face, above the hips).
        if s > 0.4 {
            let t0 = g.face.map_or(by + bh / 4, |f| f.1 + (6.0 * g.sf) as i32);
            let t1 = (by + bh * 3 / 5).max(t0 + 1);
            let bars = if rng.range(0.0, 1.0) < 0.5 { 1 } else { 2 };
            for colour in [YELLOW, VIOLET].into_iter().take(bars) {
                let y = rng.range(t0 as f32, t1 as f32) as i32;
                let hh = (rng.range(3.0, 6.0) * g.sf) as u16;
                let x0 = bx + (rng.range(-0.12, 0.25) * bw as f32) as i32;
                let w = (rng.range(0.6, 1.0) * bw as f32) as u16;
                self.cells.push(((x0 as i16, y as i16, w, hh), colour));
            }
        }
        self.scan = 0.3 * s;
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

    /// `scaled` cut to the drawn bands and shifted with them (plus `dx`, `dy`). `only`:
    /// just these bands (by index).
    fn banded(&self, scaled: &[ShapeRect], dx: i32, dy: i32, only: Option<&dyn Fn(usize) -> bool>, out: &mut Vec<ShapeRect>) {
        let mv = |x: i16, y: i32, w: u16, h: i32, sx: i32, sy: i32| ((x as i32 + sx + dx) as i16, (y + sy + dy) as i16, w, h as u16);
        match &self.slices {
            None => out.extend(scaled.iter().map(|&(x, y, w, h)| mv(x, y as i32, w, h as i32, 0, 0))),
            Some(bands) => {
                for &(x, y, w, h) in scaled {
                    let (ry0, ry1) = (y as i32, y as i32 + h as i32);
                    for (i, &(b0, b1, sx, sy)) in bands.iter().enumerate() {
                        if only.is_some_and(|f| !f(i)) {
                            continue;
                        }
                        let (y0, y1) = (ry0.max(b0), ry1.min(b1));
                        if y1 > y0 {
                            out.push(mv(x, y0, w, y1 - y0, sx, sy));
                        }
                    }
                }
            }
        }
    }

    /// The body as drawn (squashed, banded, shifted), no ghosts or cells.
    pub fn body_rects(&self, rects: &[ShapeRect]) -> Vec<ShapeRect> {
        let mut out = Vec::new();
        self.banded(&self.scaled(rects), 0, 0, None, &mut out);
        out
    }

    /// Bands drawn as a solid colour: `(rect, rgb)`.
    pub fn solid_rects(&self, rects: &[ShapeRect]) -> Vec<(ShapeRect, u32)> {
        let scaled = self.scaled(rects);
        let mut out = Vec::new();
        let mut tmp = Vec::new();
        for (i, c) in self.solid.iter().enumerate() {
            let Some(c) = *c else { continue };
            tmp.clear();
            self.banded(&scaled, 0, 0, Some(&|j| j == i), &mut tmp);
            out.extend(tmp.iter().map(|&r| (r, c)));
        }
        out
    }

    /// The red/blue split silhouettes, band by band: `(rect, rgb)`.
    pub fn ghost_rects(&self, rects: &[ShapeRect]) -> Vec<(ShapeRect, u32)> {
        let scaled = self.scaled(rects);
        let mut out = Vec::new();
        let mut tmp = Vec::new();
        for (i, &d) in self.split.iter().enumerate() {
            if d == 0 {
                continue;
            }
            for (off, c) in [(-d, SPLIT_RED), (d, SPLIT_BLUE)] {
                tmp.clear();
                self.banded(&scaled, off, 0, Some(&|j| j == i), &mut tmp);
                out.extend(tmp.iter().map(|&r| (r, c)));
            }
        }
        out
    }

    /// Everything this plan draws, as shape rects (avatar-local).
    pub fn shape(&self, rects: &[ShapeRect]) -> Vec<ShapeRect> {
        let scaled = self.scaled(rects);
        let mut out = Vec::with_capacity(scaled.len() * 3);
        self.banded(&scaled, 0, 0, None, &mut out);
        out.extend(self.ghost_rects(rects).into_iter().map(|g| g.0));
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

/// Idle and music body motion, device px: `breath` lifts everything above the waist,
/// `nod` drops the head, `sway` shears the top of the figure sideways (full at the top
/// row, nothing at the shear line).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Pose {
    pub breath: i32,
    pub nod: i32,
    pub sway: i32,
}

/// Rows (avatar-local) a pose acts on.
#[derive(Clone, Copy, Debug)]
pub struct PoseRows {
    /// First row to band (negative reaches the HUD area; nothing to draw there).
    pub y0: i32,
    pub h: i32,
    /// Topmost opaque row.
    pub top: i32,
    /// Rows above this shear with `sway`.
    pub shear_end: i32,
    /// Rows above this nod.
    pub neck: i32,
    /// Rows above this breathe.
    pub waist: i32,
}

/// Bands `(y0, y1, dx, dy)` in paint order (legs, torso, head) for a pose. The waist rows
/// repeat over the breath height, so nothing opens up between torso and legs.
pub fn pose_bands(p: Pose, r: PoseRows) -> Vec<(i32, i32, i32, i32)> {
    let b = p.breath.max(0);
    let waist = r.waist.clamp(r.y0 + 1, r.h);
    let neck = r.neck.clamp(r.y0, waist);
    let shear_end = r.shear_end.clamp(r.y0, waist);
    let span = (shear_end - r.top).max(1) as f32;
    let shear = |y: i32| {
        let f = ((shear_end - y) as f32 / span).clamp(0.0, 1.0);
        (p.sway as f32 * f.powf(1.2)).round() as i32
    };
    // Legs first, then the torso, then the head on top (a nod overlaps the collar).
    let mut out = vec![(waist - b, r.h, 0, 0)];
    for (from, to, dy) in [(neck, waist, -b), (r.y0, neck, p.nod - b)] {
        let first = out.len();
        let mut y = from;
        while y < to {
            let (y1, dx) = if p.sway != 0 && y < shear_end { ((y + 2).min(shear_end).min(to), shear(y)) } else { (to, 0) };
            let n = out.len();
            match out.last_mut() {
                Some(last) if n > first && last.1 == y && last.2 == dx && last.3 == dy => last.1 = y1,
                _ => out.push((y, y1, dx, dy)),
            }
            y = y1;
        }
    }
    out
}

/// Capri's note particle noise.
fn noise(n: u64, seed: u32) -> f32 {
    let v = (((n + 1) as f32) * 12.9898 + seed as f32 * 78.233).sin() * 43758.547;
    v - v.floor()
}

fn smooth(a: f32, b: f32, v: f32) -> f32 {
    let t = ((v - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn cubic(a: f32, b: f32, c: f32, d: f32, t: f32) -> f32 {
    let u = 1.0 - t;
    u * u * u * a + 3.0 * u * u * t * b + 3.0 * u * t * t * c + t * t * t * d
}

/// One floating note, in the music frame's canvas px.
#[derive(Clone, Copy, Debug)]
pub struct Note {
    pub sprite: usize,
    pub x: f32,
    pub y: f32,
    pub size: f32,
    pub alpha: f32,
    pub deg: f32,
}

/// Notes alive `age_s` into a session: one born per beat from alternating emitters, each
/// drifting out and up along a curve for 2.5–3.15 s, growing in, fading out, rocking.
/// (Capri's music-animation behaviour; at most seven.)
pub fn music_notes(age_s: f32, beat_s: f32, emitters: &[[i32; 2]]) -> Vec<Note> {
    let mut out = Vec::new();
    if emitters.is_empty() || beat_s <= 0.0 {
        return out;
    }
    let last = (age_s / beat_s).floor() as i64;
    let first = (((age_s - 3.15) / beat_s).floor() as i64).max(0);
    for n in first..=last {
        let n = n as u64;
        let at = n as f32 * beat_s;
        let life = 2.5 + noise(n, 1) * 0.65;
        let p = (age_s - at) / life;
        if p <= 0.0 || p >= 1.0 {
            continue;
        }
        let side = (n % 2) as usize;
        let dir = if side == 1 { 1.0 } else { -1.0 };
        let [ex, ey] = emitters[side.min(emitters.len() - 1)];
        let (ex, ey) = (ex as f32, ey as f32);
        let dx = dir * (270.0 + 60.0 * noise(n, 2));
        let dy = -(185.0 + 65.0 * noise(n, 3));
        out.push(Note {
            sprite: ((n / 2) as usize + side) % 4,
            x: cubic(ex, ex + dir * 74.0, ex + dx * 0.85, ex + dx, p),
            y: ey + cubic(0.0, -14.0, dy * 0.52, dy, p),
            size: (92.0 + 28.0 * noise(n, 4)) * (0.62 + 0.38 * smooth(0.0, 0.22, p)),
            alpha: smooth(0.0, 0.16, p) * (1.0 - smooth(0.68, 1.0, p)),
            deg: dir * ((noise(n, 5) - 0.5) * 8.0 + (p * PI).sin() * 14.0),
        });
    }
    if out.len() > 7 {
        out.drain(..out.len() - 7);
    }
    out
}

/// Breathing bob, device px upward: 0..=px on a cosine.
pub fn bob(elapsed: Duration, px: i32, period_ms: u64) -> i32 {
    if px <= 0 || period_ms == 0 {
        return 0;
    }
    let ph = ms(elapsed) / period_ms as f32 * 2.0 * PI;
    (px as f32 * (0.5 - 0.5 * ph.cos())).round() as i32
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
/// The body lands (window moves, still hidden) when the tip reaches the target…
pub const BOLT_ARRIVE: f32 = 0.4;
/// …and the landing glitch takes over as the trail drains into it.
pub const BOLT_END: f32 = 0.62;
/// How the travelling line looks. Both travel the same way (the tip races from the
/// origin to the target with the trail behind it); only the drawing differs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LineStyle {
    /// One flat red ribbon (the bar's bolt red); no core, glow, split or flicker.
    Plain,
    /// A red bolt with a light core, two stacked glow halos and a blue/cyan split; the
    /// zig-zag, width, glow and forks re-roll every frame.
    Glow,
}

impl LineStyle {
    pub fn parse(s: &str) -> Option<LineStyle> {
        match s {
            "plain" => Some(LineStyle::Plain),
            "glow" => Some(LineStyle::Glow),
            _ => None,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            LineStyle::Plain => "plain",
            LineStyle::Glow => "glow",
        }
    }
}

/// Plain: the bar's bolt red.
pub const BOLT_LINE: u32 = 0xff1738;
/// Glow layers, outside in: lighter toward the core, no dark outline.
pub const BOLT_GLOW: [u32; 4] = [0xe8163c, 0xff5274, 0xffa3b6, 0xfff4f7];
/// Glow: the split ghosts either side of the bolt.
pub const BOLT_SPLIT: [u32; 2] = [0x2e7bff, 0x00ffdd];
/// Glow: the zig-zag re-rolls every frame at this period.
const BOLT_STEP_MS: f32 = 33.0;

/// One bolt layer: colour, edge grid (device px), polygons (root px).
pub struct BoltLayer {
    pub rgb: u32,
    pub grid: i32,
    pub polys: Vec<Vec<(f32, f32)>>,
}

#[derive(Clone, Copy, Debug)]
struct RoutePt {
    x: f32,
    y: f32,
    t: f32,
}

/// A jagged strike from `a` to `b` (root px).
pub struct Bolt {
    a: (f32, f32),
    axis: (f32, f32),
    len: f32,
    seed: u64,
    scale: f32,
    style: LineStyle,
    bbox: (i32, i32, i32, i32),
}

fn along(p: (f32, f32), axis: (f32, f32), fwd: f32, across: f32) -> (f32, f32) {
    (p.0 + axis.0 * fwd - axis.1 * across, p.1 + axis.1 * fwd + axis.0 * across)
}

impl Bolt {
    /// `scale`: figure size in px (bolt thickness and branch reach follow it).
    pub fn new(a: (f32, f32), b: (f32, f32), seed: u64, scale: f32, style: LineStyle) -> Bolt {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len = (dx * dx + dy * dy).sqrt().max(1.0);
        let axis = (dx / len, dy / len);
        let amp = (len * 0.11).clamp(16.0, 90.0);
        let pad = scale * 0.6 + amp * 1.6 + 24.0;
        let (x0, x1) = (a.0.min(b.0) - pad, a.0.max(b.0) + pad);
        let (y0, y1) = (a.1.min(b.1) - pad, a.1.max(b.1) + pad);
        Bolt { a, axis, len, seed, scale, style, bbox: (x0 as i32, y0 as i32, (x1 - x0) as i32, (y1 - y0) as i32) }
    }

    fn amp(&self) -> f32 {
        (self.len * 0.11).clamp(16.0, 90.0)
    }

    /// The zig-zag route. `step`: re-jitter the elbows for this frame (glow); `None` =
    /// the plain route (the jitter's midpoint), the same every frame.
    fn route(&self, step: Option<u64>) -> Vec<RoutePt> {
        let mut rng = Rng::new(self.seed | 1);
        let mut jit = step.map(|st| Rng::new((self.seed ^ st.wrapping_mul(0x9e37_79b9_7f4a_7c15)) | 1));
        let mut j = |lo: f32, hi: f32| jit.as_mut().map_or((lo + hi) / 2.0, |r| r.range(lo, hi));
        let amp = self.amp();
        let a = self.a;
        let mirror = if rng.range(0.0, 1.0) < 0.5 { -1.0 } else { 1.0 };
        let mut route = vec![RoutePt { x: a.0, y: a.1, t: 0.0 }];
        for &(base, off) in &[(0.13, -0.3), (0.27, 0.66), (0.41, -0.6), (0.55, 0.78), (0.69, -0.26), (0.8, 0.7), (0.91, -0.16)] {
            let t = base + rng.range(-0.02, 0.02) + j(-0.012, 0.012);
            let bend = off * amp * mirror * rng.range(0.73, 1.18) * j(0.7, 1.3);
            let prev = *route.last().expect("route start");
            let corner = along(a, self.axis, t * self.len, bend);
            let crack = rng.range(0.18, 0.34) * amp * 0.6 * j(0.6, 1.4);
            let mid = ((prev.x + (corner.0 - prev.x) * 0.62), (prev.y + (corner.1 - prev.y) * 0.62));
            let mid = along(mid, self.axis, 0.0, -bend.signum() * crack);
            route.push(RoutePt { x: mid.0, y: mid.1, t: prev.t + (t - prev.t) * 0.62 });
            route.push(RoutePt { x: corner.0, y: corner.1, t });
        }
        let b = along(a, self.axis, self.len, 0.0);
        route.push(RoutePt { x: b.0, y: b.1, t: 1.0 });
        route
    }

    fn at(route: &[RoutePt], t: f32) -> (f32, f32) {
        let t = t.clamp(0.0, 1.0);
        let i = route.iter().position(|p| p.t >= t).unwrap_or(route.len() - 1).max(1);
        let (a, b) = (route[i - 1], route[i]);
        let u = if b.t > a.t { (t - a.t) / (b.t - a.t) } else { 0.0 };
        (a.x + (b.x - a.x) * u, a.y + (b.y - a.y) * u)
    }

    pub fn bbox(&self) -> (i32, i32, i32, i32) {
        self.bbox
    }

    /// Tip and tail along the route at time `u` (0..1 of BOLT_MS): the tip races from the
    /// origin to the target in ~170 ms with the trail behind it, then the trail drains
    /// into the target.
    pub fn span(u: f32) -> (f32, f32) {
        let ease = |v: f32| {
            let v = v.clamp(0.0, 1.0);
            1.0 - (1.0 - v) * (1.0 - v)
        };
        (ease(0.06 + u / 0.4), ease((u - 0.13) / 0.47))
    }

    /// The bolt at time `u`, bottom layer first (empty before/after the strike).
    /// `grid`: edge grid for the glow halos, device px.
    pub fn frame(&self, u: f32, grid: i32) -> Vec<BoltLayer> {
        let (head, tail) = Bolt::span(u);
        if head <= tail + 0.004 {
            return Vec::new();
        }
        let glow = self.style == LineStyle::Glow;
        let step = (u * BOLT_MS / BOLT_STEP_MS) as u64;
        let route = self.route(glow.then_some(step));
        let mut fl = Rng::new((self.seed ^ 0xb017 ^ step.wrapping_mul(0x2545_f491_4f6c_dd1d)) | 1);
        let travelled = head - tail;
        let mut seg: Vec<(f32, f32, f32)> = vec![{
            let p = Bolt::at(&route, tail);
            (p.0, p.1, tail)
        }];
        seg.extend(route.iter().filter(|p| p.t > tail && p.t < head).map(|p| (p.x, p.y, p.t)));
        let hp = Bolt::at(&route, head);
        seg.push((hp.0, hp.1, head));
        let pts: Vec<(f32, f32)> = seg.iter().map(|p| (p.0, p.1)).collect();
        let n = seg.len();
        // Thickest just behind the tip.
        let taper = |radius: f32, zig: bool| -> Vec<f32> {
            seg.iter()
                .enumerate()
                .map(|(i, p)| {
                    if i == 0 || i == n - 1 {
                        0.6
                    } else {
                        let k = if !zig { 1.0 } else if i % 2 == 1 { 1.12 } else { 0.86 };
                        radius * (0.45 + 0.6 * (p.2 - tail) / travelled) * k
                    }
                })
                .collect()
        };
        if !glow {
            let widths = taper((self.scale * 0.07).clamp(4.0, 18.0), false);
            return vec![BoltLayer { rgb: BOLT_LINE, grid: 2, polys: vec![ribbon(&pts, &widths)] }];
        }
        let widths = taper((self.scale * 0.09).clamp(5.0, 24.0) * fl.range(0.7, 1.25), true);
        let halo = fl.range(0.8, 1.2);
        let grow = |k: f32, add: f32| widths.iter().map(|w| w * k + add).collect::<Vec<_>>();
        // The split: the bolt again either side, across the strike, 4–8 px.
        let d = fl.range(4.0, 8.0) * (self.scale / 220.0).clamp(0.8, 1.6);
        let shift = |s: f32| pts.iter().map(|&p| along(p, self.axis, 0.0, s)).collect::<Vec<_>>();
        let mut out = vec![
            BoltLayer { rgb: BOLT_SPLIT[0], grid: 2, polys: vec![ribbon(&shift(d), &widths)] },
            BoltLayer { rgb: BOLT_SPLIT[1], grid: 2, polys: vec![ribbon(&shift(-d), &widths)] },
            BoltLayer { rgb: BOLT_GLOW[0], grid, polys: vec![ribbon(&pts, &grow(1.9 * halo, 4.0))] },
            BoltLayer { rgb: BOLT_GLOW[1], grid, polys: vec![ribbon(&pts, &grow(1.35 * halo, 1.5))] },
            BoltLayer { rgb: BOLT_GLOW[2], grid: 2, polys: vec![ribbon(&pts, &widths)] },
            BoltLayer { rgb: BOLT_GLOW[3], grid: 2, polys: vec![ribbon(&pts, &grow(0.38, 0.0))] },
        ];
        // 0–3 small forks off the travelled part; they come and go frame to frame.
        if travelled > 0.08 {
            for k in 0..fl.pick(4) {
                let t = tail + travelled * fl.range(0.15, 0.85);
                let side = if (k + step as usize) % 2 == 1 { 1.0 } else { -1.0 };
                let o = Bolt::at(&route, t);
                let (p0, p1) = (Bolt::at(&route, t - 0.04), Bolt::at(&route, t + 0.04));
                let (ax, ay) = (p1.0 - p0.0, p1.1 - p0.1);
                let al = (ax * ax + ay * ay).sqrt().max(1e-3);
                let axis = (ax / al, ay / al);
                let reach = self.scale * fl.range(0.12, 0.3);
                let fork = |f: f32, a: f32| along(o, axis, reach * f, side * reach * a);
                let stem = [o, fork(0.1, 0.25), fork(0.05, 0.45), fork(0.4, 0.62), fork(0.32, 0.85), fork(0.7, 1.05)];
                let th = (self.scale * 0.012).clamp(1.6, 3.6);
                let w: Vec<f32> = [1.3, 1.6, 1.25, 0.9, 0.55, 0.1].iter().map(|v| v * th).collect();
                out[3].polys.push(ribbon(&stem, &w.iter().map(|v| v * 1.6 + 1.0).collect::<Vec<_>>()));
                out[4].polys.push(ribbon(&stem, &w));
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
            solid: 0.0,
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
                    solid: g.solid(at),
                    sf: 1.0,
                });
                let bands = plan.slices.clone().unwrap();
                // Every row from the HUD to the feet is in exactly one band: nothing dropped.
                for y in -80..400 {
                    assert_eq!(bands.iter().filter(|b| y >= b.0 && y < b.1).count(), 1, "{kind:?} {ms} row {y}");
                }
                let face: Vec<usize> = (0..bands.len()).filter(|&i| bands[i].1 > 90 && bands[i].0 < 140).collect();
                assert_eq!(face.iter().map(|&i| bands[i].1 - bands[i].0).sum::<i32>(), 50, "{kind:?} {ms}");
                assert!(face.iter().all(|&i| bands[i].2.abs() <= 3 && bands[i].0 >= 90 && bands[i].1 <= 140 && plan.split[i] == 0));
                assert!(face.iter().all(|&i| plan.solid[i].is_none()));
                if g.strength(at) >= 0.8 {
                    let body = bands.iter().filter(|b| b.0 >= 10).count();
                    assert!((6..=16).contains(&body), "{kind:?} {ms}: {body} bands");
                    let shifted: Vec<i32> = bands.iter().enumerate().filter(|(i, b)| b.0 >= 10 && !face.contains(i) && b.2 != 0).map(|b| b.1.2.abs()).collect();
                    assert!(shifted.iter().all(|&d| (10..=40).contains(&d)), "{shifted:?}");
                    assert!(plan.split.iter().enumerate().filter(|(i, _)| !face.contains(i)).all(|(_, &d)| (4..=8).contains(&d)));
                }
            }
        }
    }

    #[test]
    fn big_glitch_stutters() {
        let t = Instant::now();
        let g = Glitch::new(GlitchKind::Big, t, 5);
        let v: Vec<f32> = (0..85).map(|i| g.strength(t + Duration::from_millis(i * 10))).collect();
        let flips = v.windows(2).filter(|w| (w[0] > 0.3) != (w[1] > 0.3)).count();
        assert!(flips >= 5, "{flips}");
        // Main hit 0.5–0.6 s.
        let hit = v.iter().take_while(|&&x| x > 0.3).count();
        assert!((50..=60).contains(&hit), "{hit}");
        assert!(g.ms >= 800.0);
    }

    #[test]
    fn pose_bands_cover_without_gaps() {
        let r = PoseRows { y0: -50, h: 400, top: 10, shear_end: 80, neck: 120, waist: 220 };
        let bands = pose_bands(Pose { breath: 2, nod: 0, sway: 3 }, r);
        // Every destination row from the top to the bottom is painted by some band.
        for y in 10..400 {
            assert!(bands.iter().any(|&(a, b, _, dy)| y >= a + dy && y < b + dy), "row {y} {bands:?}");
        }
        assert_eq!(bands.iter().filter(|b| b.2 == 3).count(), 1);
        assert!(bands.iter().all(|b| b.2.abs() <= 3));
        // The legs never move.
        assert!(bands.iter().filter(|b| b.1 > 220).all(|b| b.2 == 0 && b.3 == 0));
        // Music: the shear reaches down to the waist (the upper body tilts), head nods.
        let music = pose_bands(Pose { breath: 0, nod: 4, sway: 4 }, PoseRows { shear_end: 220, ..r });
        for y in 10..400 {
            assert!(music.iter().any(|&(a, b, _, dy)| y >= a + dy && y < b + dy), "row {y}");
        }
        assert!(music.iter().any(|b| b.0 >= 120 && b.1 <= 220 && b.2 != 0));
        let still = pose_bands(Pose::default(), r);
        assert!(still.iter().all(|b| b.2 == 0 && b.3 == 0));
    }

    #[test]
    fn notes_rise_and_fade() {
        let em = [[500, 600], [760, 590]];
        assert!(music_notes(0.01, 0.545, &em).len() <= 1);
        let steady = music_notes(20.0, 0.545, &em);
        assert!((4..=7).contains(&steady.len()), "{}", steady.len());
        assert!(steady.iter().all(|n| n.y < 600.0 && n.alpha >= 0.0 && n.alpha <= 1.0 && n.deg.abs() <= 18.5));
    }

    #[test]
    fn bolt_reaches_both_ends() {
        for style in [LineStyle::Plain, LineStyle::Glow] {
            let b = Bolt::new((0.0, 0.0), (400.0, 100.0), 3, 300.0, style);
            let r = b.route(None);
            assert_eq!(Bolt::at(&r, 0.0), (0.0, 0.0));
            let end = Bolt::at(&r, 1.0);
            assert!((end.0 - 400.0).abs() < 0.01 && (end.1 - 100.0).abs() < 0.01);
            let layers = b.frame(0.3, 3);
            assert_eq!(layers.len(), if style == LineStyle::Plain { 1 } else { 6 });
            assert!(layers.iter().all(|l| !l.polys.is_empty()));
            assert!(b.frame(1.0, 3).is_empty());
        }
        // Plain: one flat red, the same shape every frame (no flicker).
        let p = Bolt::new((0.0, 0.0), (400.0, 100.0), 3, 300.0, LineStyle::Plain);
        assert_eq!(p.frame(0.30, 3)[0].polys, p.frame(0.30, 3)[0].polys);
        assert_eq!(p.route(None)[3].x, p.route(None)[3].x);
        assert_eq!(p.frame(0.3, 3)[0].rgb, BOLT_LINE);
        // Glow: the zig-zag re-rolls from frame to frame.
        let g = Bolt::new((0.0, 0.0), (400.0, 100.0), 3, 300.0, LineStyle::Glow);
        assert_ne!(g.route(Some(1))[3].x, g.route(Some(2))[3].x);
        // The tip travels: it advances over 3–5 frames at 30 fps before reaching the target.
        let frames = (BOLT_MS / 33.3) as usize;
        let tips: Vec<f32> = (0..frames).map(|i| Bolt::span(i as f32 * 33.3 / BOLT_MS).0).collect();
        let racing = tips.iter().filter(|&&h| h < 0.98).count();
        assert!((3..=5).contains(&racing), "{tips:?}");
        // The trail drains before the landing takes over.
        let (h, t) = Bolt::span(BOLT_END);
        assert!(h > 0.99 && t > 0.95);
    }
}
