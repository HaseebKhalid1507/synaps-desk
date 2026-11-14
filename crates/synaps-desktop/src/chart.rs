//! Usage's charts on GPUI's `canvas()`: bars are quads, lines one stroked
//! path per run, grid lines `hair`. No chart library, no animation: a
//! chart paints when its view repaints (data, a filter, the pointer).
//! The data is laid out once per load ([`crate::usage::Board`]); a paint
//! only scales it to the plot's bounds.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::*;

use crate::usage::Stack;

/// Where a plot was painted last (the pointer's day is read from it).
pub type Spot = Rc<Cell<Option<Bounds<Pixels>>>>;

/// A y axis: linear between `lo` and `hi`, or logarithmic (natural logs of
/// the bounds).
#[derive(Clone, Copy, Debug)]
pub enum Scale {
    Linear(f64, f64),
    Log(f64, f64),
}

impl Scale {
    /// `v` as 0 (bottom) to 1 (top), clamped.
    pub fn at(self, v: f64) -> f32 {
        let (x, lo, hi) = match self {
            Scale::Linear(lo, hi) => (v, lo, hi),
            Scale::Log(lo, hi) => (v.max(1e-9).ln(), lo, hi),
        };
        if hi <= lo { 0. } else { ((x - lo) / (hi - lo)).clamp(0., 1.) as f32 }
    }

    /// The scale for these lines: from 0 (linear) or their lowest point
    /// (log) to their highest.
    pub fn log_of(s: &Stack) -> Scale {
        let (lo, hi) = s
            .series
            .iter()
            .flat_map(|l| l.values.iter().flatten())
            .filter(|v| **v > 0.)
            .fold((f64::MAX, 0f64), |(lo, hi), &v| (lo.min(v), hi.max(v)));
        if hi <= 0. { Scale::Log(0., 1.) } else { Scale::Log((lo * 0.8).ln(), (hi * 1.1).ln()) }
    }
}

/// The colours a chart paints with.
#[derive(Clone, Copy)]
pub struct Ink {
    pub series: [Hsla; 6],
    pub grid: Hsla,
    pub rule: Hsla,
    pub hover: Hsla,
}

/// The day the pointer is over, from where the plot was painted.
pub fn day_at(spot: &Spot, s: &Stack, x: Pixels) -> Option<usize> {
    let b = spot.get()?;
    let w = f32::from(b.size.width);
    (w > 0.).then(|| s.index_at((f32::from(x) - f32::from(b.origin.x)) / w)).flatten()
}

fn grid_lines(b: Bounds<Pixels>, ticks: &[f32], ink: &Ink, window: &mut Window) {
    for &t in ticks {
        let y = b.origin.y + b.size.height * (1. - t);
        window.paint_quad(fill(Bounds::new(point(b.origin.x, y.floor()), size(b.size.width, px(1.))), ink.grid));
    }
    // The base line.
    window.paint_quad(fill(Bounds::new(point(b.origin.x, b.origin.y + b.size.height - px(1.)), size(b.size.width, px(1.))), ink.grid));
}

/// A dotted vertical rule at day `ix` (request-level data from here).
fn rule(b: Bounds<Pixels>, n: usize, ix: usize, color: Hsla, window: &mut Window) {
    let x = b.origin.x + b.size.width * (ix as f32 / n as f32);
    let mut p = PathBuilder::stroke(px(1.)).dash_array(&[px(2.), px(3.)]);
    p.move_to(point(x.floor() + px(0.5), b.origin.y));
    p.line_to(point(x.floor() + px(0.5), b.origin.y + b.size.height));
    if let Ok(path) = p.build() {
        window.paint_path(path, color);
    }
}

fn hover_column(b: Bounds<Pixels>, n: usize, ix: usize, color: Hsla, window: &mut Window) {
    let w = b.size.width / n as f32;
    window.paint_quad(fill(Bounds::new(point(b.origin.x + w * ix as f32, b.origin.y), size(w.max(px(1.)), b.size.height)), color));
}

/// Stacked daily bars from 0 to `s.max`; `ticks`: grid lines (0..1).
pub fn bars(s: Arc<Stack>, ink: Ink, ticks: Vec<f32>, hover: Option<usize>, request_ix: Option<usize>, spot: Spot) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |b, _, window, _| {
            spot.set(Some(b));
            let n = s.days.len();
            grid_lines(b, &ticks, &ink, window);
            if n == 0 || s.max <= 0. {
                return;
            }
            if let Some(h) = hover {
                hover_column(b, n, h, ink.hover, window);
            }
            let w = f32::from(b.size.width) / n as f32;
            let gap = if w >= 4. { 1. } else { 0. };
            let h = f32::from(b.size.height);
            for i in 0..n {
                let x = f32::from(b.origin.x) + w * i as f32;
                let mut y = f32::from(b.origin.y) + h;
                for l in &s.series {
                    let v = l.values[i].unwrap_or(0.);
                    if v <= 0. {
                        continue;
                    }
                    let bh = (v / s.max) as f32 * h;
                    y -= bh;
                    window.paint_quad(fill(
                        Bounds::new(point(px(x), px(y)), size(px((w - gap).max(1.)), px(bh.max(0.5)))),
                        ink.series[l.color.min(5)],
                    ));
                }
            }
            if let Some(r) = request_ix {
                rule(b, n, r, ink.rule, window);
            }
        },
    )
    .size_full()
}

/// One line per series on `scale`; a `None` breaks it (a gap, never a
/// dip). `thin`: series drawn 1 px in their colour at half strength (p95).
/// `dim_until`: days before it are drawn in the grid colour (legacy data).
/// `marks`: x-axis ticks (days) drawn as short `rule` lines at the foot.
#[allow(clippy::too_many_arguments)]
pub fn lines(
    s: Arc<Stack>,
    scale: Scale,
    ink: Ink,
    ticks: Vec<f32>,
    thin: Vec<bool>,
    dim_until: Option<usize>,
    marks: Vec<usize>,
    hover: Option<usize>,
    spot: Spot,
) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |b, _, window, _| {
            spot.set(Some(b));
            let n = s.days.len();
            grid_lines(b, &ticks, &ink, window);
            if n == 0 {
                return;
            }
            if let Some(h) = hover {
                hover_column(b, n, h, ink.hover, window);
            }
            let step = b.size.width / n as f32;
            let x_of = |i: usize| b.origin.x + step * (i as f32 + 0.5);
            let y_of = |v: f64| b.origin.y + b.size.height * (1. - scale.at(v));
            for &m in &marks {
                let x = x_of(m).floor();
                window.paint_quad(fill(Bounds::new(point(x, b.origin.y + b.size.height - px(6.)), size(px(1.), px(6.))), ink.rule));
            }
            for (si, l) in s.series.iter().enumerate() {
                let thin = thin.get(si).copied().unwrap_or(false);
                let base = ink.series[l.color.min(5)];
                let color = if thin { base.opacity(0.5) } else { base };
                // One path per run of known days; a lone day is a dot.
                let mut i = 0;
                while i < n {
                    if l.values[i].is_none() {
                        i += 1;
                        continue;
                    }
                    let start = i;
                    while i < n && l.values[i].is_some() {
                        i += 1;
                    }
                    let run = start..i;
                    let paint = |from: usize, to: usize, c: Hsla, window: &mut Window| {
                        if to - from == 1 {
                            let (x, y) = (x_of(from), y_of(l.values[from].unwrap_or(0.)));
                            window.paint_quad(fill(Bounds::new(point(x - px(1.5), y - px(1.5)), size(px(3.), px(3.))), c));
                            return;
                        }
                        let mut p = PathBuilder::stroke(px(if thin { 1. } else { 1.5 }));
                        p.move_to(point(x_of(from), y_of(l.values[from].unwrap_or(0.))));
                        for (j, v) in l.values.iter().enumerate().take(to).skip(from + 1) {
                            p.line_to(point(x_of(j), y_of(v.unwrap_or(0.))));
                        }
                        if let Ok(path) = p.build() {
                            window.paint_path(path, c);
                        }
                    };
                    match dim_until.filter(|&d| d > run.start) {
                        Some(d) if d < run.end => {
                            paint(run.start, d + 1, ink.grid, window);
                            paint(d, run.end, color, window);
                        }
                        Some(_) => paint(run.start, run.end, ink.grid, window),
                        None => paint(run.start, run.end, color, window),
                    }
                }
            }
        },
    )
    .size_full()
}

/// 2-px squares at half strength; points already on 0..1 both ways.
pub fn scatter(points: Arc<[(f32, f32)]>, color: Hsla, grid: Hsla, ticks: Vec<f32>) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |b, _, window, _| {
            let ink = Ink { series: [color; 6], grid, rule: grid, hover: grid };
            grid_lines(b, &ticks, &ink, window);
            let c = color.opacity(0.5);
            let (w, h) = (b.size.width - px(2.), b.size.height - px(2.));
            for &(x, y) in points.iter() {
                window.paint_quad(fill(Bounds::new(point(b.origin.x + w * x, b.origin.y + h * (1. - y)), size(px(2.), px(2.))), c));
            }
        },
    )
    .size_full()
}

#[cfg(test)]
mod tests {
    use super::Scale;

    #[test]
    fn scales_clamp_and_log() {
        assert_eq!(Scale::Linear(50., 100.).at(75.), 0.5);
        assert_eq!(Scale::Linear(50., 100.).at(10.), 0.);
        assert_eq!(Scale::Linear(1., 1.).at(5.), 0.);
        let l = Scale::Log(1f64.ln(), 100f64.ln());
        assert!((l.at(10.) - 0.5).abs() < 1e-6);
        assert_eq!(l.at(0.), 0.);
    }
}
