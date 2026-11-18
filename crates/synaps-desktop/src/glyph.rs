//! The one drawn glyph set and the state table (design.md §4). Every state
//! and every affordance is drawn here, in one 10×10 box: quads (absolute
//! divs) for the silhouettes, and for the diagonals (the tick, the cross,
//! the chevrons) a stroked SVG sprite, rasterized into GPUI's sprite atlas
//! once per size and scale and then drawn as a tinted monochrome sprite,
//! batched with the text (a GPUI path would cost a full-window MSAA pass on
//! every frame it is on). Never a font character: the embedded fonts are
//! subsets, and Plex Mono's `✓` reads as `√` at 13 px. A glyph takes its
//! family's mark role on its ground, its word the family's text role
//! ([`ink`]).
//!
//! Every surface that shows a state reads this table (design.md §4.3): the
//! rail, the tabs, the composer foot, the tray, the worker blocks, the
//! hover card and the NEEDS YOU stub.

use std::borrow::Cow;
use std::fmt::Write;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use synaps_client::transcript::ToolStatus;

use crate::fonts::label;
use crate::rail::State;
use crate::subagents::Liveness;
use crate::theme::{Metrics, Palette};

/// Every drawing (design.md §4.1). Each one is unique, so a state reads by
/// its shape where two families share a colour (`run` and `warn` are one
/// colour in 7 themes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Glyph {
    Run,
    Workers,
    Needs,
    Unread,
    Idle,
    Parked,
    Opening,
    Locked,
    Gone,
    Cancelling,
    Cancelled,
    Limit,
    Ok,
    Failed,
    /// A closed fold; also [`GO`].
    Fold,
    FoldOpen,
    /// Close or cancel: `Failed` in an 8 px box.
    Close,
    /// Favourite this session (a row's reveal): a five-point star's
    /// outline, stroke 1, round joins.
    Star,
    /// A favourite (desktop only, never the daemon's pin): the star filled.
    Starred,
    /// The daemon pins it (kept loaded, never parked), on a favourite's
    /// row: a push pin side on (its cap, neck, flange and needle).
    Pinned,
    /// Open a worker's view (a spawn row's OPEN button): ↗, stroked.
    Open,
    /// A worker's live step under its spawn row: ↳, two quads.
    Elbow,
}

/// Open it (in a tab): the closed fold's chevron.
pub(crate) const GO: Glyph = Glyph::Fold;

/// One part of a drawing, in px from its box's top-left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Part {
    /// A filled rectangle: x, y, w, h.
    Fill(f32, f32, f32, f32),
    /// A 1 px frame, its outer rectangle: x, y, w, h.
    Frame(f32, f32, f32, f32),
    /// A stroked polyline.
    Line(&'static [(f32, f32)]),
    /// A closed polyline (its last point its first), filled and stroked.
    Poly(&'static [(f32, f32)]),
}

/// The star's ten points (outer radius 4.4 about (5, 5.25), inner 2), top
/// first, clockwise; closed by the drawing.
const STAR: &[(f32, f32)] =
    &[(5., 0.85), (6.18, 3.63), (9.18, 3.89), (6.9, 5.87), (7.59, 8.81), (5., 7.25), (2.41, 8.81), (3.1, 5.87), (0.82, 3.89), (3.82, 3.63)];

impl Glyph {
    pub(crate) const ALL: [Glyph; 22] = [
        Glyph::Run,
        Glyph::Workers,
        Glyph::Needs,
        Glyph::Unread,
        Glyph::Idle,
        Glyph::Parked,
        Glyph::Opening,
        Glyph::Locked,
        Glyph::Gone,
        Glyph::Cancelling,
        Glyph::Cancelled,
        Glyph::Limit,
        Glyph::Ok,
        Glyph::Failed,
        Glyph::Fold,
        Glyph::FoldOpen,
        Glyph::Close,
        Glyph::Star,
        Glyph::Starred,
        Glyph::Pinned,
        Glyph::Open,
        Glyph::Elbow,
    ];

    /// Its box: 10 px; `Close` 8.
    pub(crate) fn size(self) -> f32 {
        if self == Glyph::Close { 8. } else { 10. }
    }

    fn stroke(self) -> f32 {
        if matches!(self, Glyph::Close | Glyph::Star | Glyph::Starred) { 1. } else { 1.5 }
    }

    /// A stroked glyph's sprite: its SVG's asset path ([`Sprites`]).
    pub(crate) fn sprite(self) -> Option<&'static str> {
        Some(match self {
            Glyph::Ok => "glyph/ok.svg",
            Glyph::Failed => "glyph/failed.svg",
            Glyph::Fold => "glyph/fold.svg",
            Glyph::FoldOpen => "glyph/fold-open.svg",
            Glyph::Close => "glyph/close.svg",
            Glyph::Star => "glyph/star.svg",
            Glyph::Starred => "glyph/starred.svg",
            Glyph::Open => "glyph/open.svg",
            _ => return None,
        })
    }

    pub(crate) fn parts(self) -> &'static [Part] {
        use Part::{Fill, Frame, Line, Poly};
        match self {
            // The braille family (polish.md P1): each still frame's lit
            // dots (the unlit ones are drawn on their ground, [`spin`]).
            Glyph::Run => &[Fill(2., 1., 2., 2.), Fill(6., 1., 2., 2.), Fill(6., 4., 2., 2.), Fill(6., 7., 2., 2.)],
            Glyph::Workers => &[Fill(2., 1., 2., 2.), Fill(6., 7., 2., 2.)],
            Glyph::Needs => &[Frame(0., 0., 10., 10.), Fill(3., 3., 4., 4.)],
            Glyph::Unread => &[Fill(3., 3., 4., 4.)],
            Glyph::Idle => &[Frame(1., 1., 8., 8.)],
            Glyph::Parked => &[Fill(1., 7., 8., 2.)],
            Glyph::Opening => &[Fill(2., 1., 2., 2.)],
            Glyph::Locked => &[Frame(1., 1., 8., 8.), Fill(1., 4., 8., 2.)],
            Glyph::Gone => &[Fill(0., 0., 2., 2.), Fill(8., 0., 2., 2.), Fill(0., 8., 2., 2.), Fill(8., 8., 2., 2.)],
            Glyph::Cancelling => &[Fill(2., 1., 2., 8.), Fill(6., 1., 2., 8.)],
            Glyph::Cancelled => &[Fill(1., 4., 8., 2.)],
            Glyph::Limit => &[Fill(4., 1., 2., 5.), Fill(4., 8., 2., 2.)],
            Glyph::Ok => &[Line(&[(1.5, 5.5), (4., 8.), (8.5, 2.)])],
            Glyph::Failed => &[Line(&[(2., 2.), (8., 8.)]), Line(&[(8., 2.), (2., 8.)])],
            Glyph::Fold => &[Line(&[(3.5, 1.5), (7., 5.), (3.5, 8.5)])],
            Glyph::FoldOpen => &[Line(&[(1.5, 3.5), (5., 7.), (8.5, 3.5)])],
            Glyph::Close => &[Line(&[(1.6, 1.6), (6.4, 6.4)]), Line(&[(6.4, 1.6), (1.6, 6.4)])],
            Glyph::Star => &[Line(STAR_CLOSED)],
            Glyph::Starred => &[Poly(STAR_CLOSED)],
            Glyph::Pinned => &[Fill(3., 1., 4., 2.), Fill(4., 3., 2., 2.), Fill(2., 5., 6., 1.), Fill(4., 6., 2., 3.)],
            Glyph::Open => &[Line(&[(2., 8.), (8., 2.)]), Line(&[(3.5, 2.), (8., 2.), (8., 6.5)])],
            Glyph::Elbow => &[Fill(2., 1., 2., 6.), Fill(2., 6., 7., 2.)],
        }
    }
}

/// `g` in `mark` on `ground`: a braille glyph's still frame with its
/// unlit dots ([`spin`]), else [`draw`].
pub(crate) fn draw_on(g: Glyph, mark: Hsla, ground: Hsla) -> AnyElement {
    if g.spins() { spin(g, None, mark, ground).into_any_element() } else { draw(g, mark) }
}

/// `g` in `color`: its quads, or its sprite tinted. A braille glyph: its
/// still frame's lit dots only ([`draw_on`] draws the silhouette).
pub(crate) fn draw(g: Glyph, color: Hsla) -> AnyElement {
    let s = px(g.size());
    if let Some(path) = g.sprite() {
        return svg().path(path).flex_none().size(s).text_color(color).into_any_element();
    }
    let mut el = div().relative().flex_none().size(s);
    for &part in g.parts() {
        el = match part {
            Part::Fill(x, y, w, h) => el.child(div().absolute().left(px(x)).top(px(y)).w(px(w)).h(px(h)).bg(color)),
            Part::Frame(x, y, w, h) => el.child(div().absolute().left(px(x)).top(px(y)).w(px(w)).h(px(h)).border_1().border_color(color)),
            // A stroked glyph is a sprite.
            Part::Line(_) | Part::Poly(_) => el,
        };
    }
    el.into_any_element()
}

/// [`STAR`] closed: the outline's polyline.
const STAR_CLOSED: &[(f32, f32)] = &{
    let mut out = [(0., 0.); 11];
    let mut i = 0;
    while i < 10 {
        out[i] = STAR[i];
        i += 1;
    }
    out[10] = STAR[0];
    out
};

/// A stroked glyph's SVG, written from its drawing: the box as the view
/// box, its lines as one path. Butt caps and miter joins, as a stroke's
/// defaults.
#[cfg(test)]
pub(crate) fn svg_of(g: Glyph) -> String {
    svg_cut(g, CUTS)
}

/// A stroked glyph's write-on is cut into this many sprites (each
/// rasterized once per size and scale, as the whole one is): `k` of them
/// draws its strokes, in order, to `k / CUTS` of their length.
pub(crate) const CUTS: u32 = 8;

/// [`svg_of`] drawn to `k / CUTS` of its strokes' length (motion.md §6.1:
/// the write-on; `Failed`'s two strokes one after the other).
pub(crate) fn svg_cut(g: Glyph, k: u32) -> String {
    let s = g.size();
    let seg = |a: (f32, f32), b: (f32, f32)| ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
    let lines = || g.parts().iter().filter_map(|p| if let Part::Line(pts) | Part::Poly(pts) = p { Some(*pts) } else { None });
    let poly = g.parts().iter().any(|p| matches!(p, Part::Poly(_)));
    // A closed outline (the stars) is closed once drawn whole, and joined
    // round: no corner to miter out of its box.
    let closed = |pts: &[(f32, f32)]| pts.len() > 2 && pts.first() == pts.last();
    let total: f32 = lines().map(|pts| pts.windows(2).map(|w| seg(w[0], w[1])).sum::<f32>()).sum();
    let mut left = total * k.min(CUTS) as f32 / CUTS as f32;
    let mut d = String::new();
    for pts in lines() {
        if left <= 1e-4 {
            break;
        }
        let _ = write!(d, "M{} {}", pts[0].0, pts[0].1);
        for (i, w) in pts.windows(2).enumerate() {
            let l = seg(w[0], w[1]);
            if left >= l - 1e-4 {
                if closed(pts) && i == pts.len() - 2 {
                    d.push('Z');
                } else {
                    let _ = write!(d, "L{} {}", w[1].0, w[1].1);
                }
                left -= l;
            } else {
                let t = left / l;
                let (x, y) = (w[0].0 + (w[1].0 - w[0].0) * t, w[0].1 + (w[1].1 - w[0].1) * t);
                let _ = write!(d, "L{} {}", (x * 100.).round() / 100., (y * 100.).round() / 100.);
                left = 0.;
                break;
            }
        }
    }
    let join = if lines().any(closed) { r#" stroke-linejoin="round""# } else { "" };
    let fill = if poly && k >= CUTS { "#000" } else { "none" };
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{s}" height="{s}" viewBox="0 0 {s} {s}"><path d="{d}" fill="{fill}" stroke="#000" stroke-width="{}"{join}/></svg>"##,
        g.stroke()
    )
}

/// A cut sprite's path: `glyph/ok.svg` cut at `k` is `glyph/ok~k.svg`.
/// A static table: a write-on frame formats nothing.
fn cut_path(g: Glyph, k: u32) -> Option<&'static str> {
    const _: () = assert!(CUTS == 8, "one path per cut below");
    macro_rules! cuts {
        ($stem:literal) => {
            &[
                concat!($stem, "~1.svg"),
                concat!($stem, "~2.svg"),
                concat!($stem, "~3.svg"),
                concat!($stem, "~4.svg"),
                concat!($stem, "~5.svg"),
                concat!($stem, "~6.svg"),
                concat!($stem, "~7.svg"),
            ]
        };
    }
    let base = g.sprite()?;
    if k >= CUTS {
        return Some(base);
    }
    let table: &[&str; 7] = match g {
        Glyph::Ok => cuts!("glyph/ok"),
        Glyph::Failed => cuts!("glyph/failed"),
        Glyph::Fold => cuts!("glyph/fold"),
        Glyph::FoldOpen => cuts!("glyph/fold-open"),
        Glyph::Close => cuts!("glyph/close"),
        Glyph::Star => cuts!("glyph/star"),
        Glyph::Starred => cuts!("glyph/starred"),
        Glyph::Open => cuts!("glyph/open"),
        _ => return None,
    };
    table.get(k.checked_sub(1)? as usize).copied()
}

/// The glyph and cut a sprite path names.
fn parse_cut(path: &str) -> Option<(Glyph, u32)> {
    if let Some(g) = Glyph::ALL.into_iter().find(|g| g.sprite() == Some(path)) {
        return Some((g, CUTS));
    }
    let (stem, k) = path.strip_suffix(".svg")?.rsplit_once('~')?;
    let k: u32 = k.parse().ok().filter(|k| (1..CUTS).contains(k))?;
    let g = Glyph::ALL.into_iter().find(|g| g.sprite().is_some_and(|s| s.strip_suffix(".svg") == Some(stem)))?;
    Some((g, k))
}

/// `g` drawn to `p` (0..1) in `color` (motion.md §2.2 `draw_at`): strokes
/// cut at `p` of their length, quads at `p` of their width. 1: [`draw`].
pub(crate) fn draw_at(g: Glyph, p: f32, color: Hsla) -> AnyElement {
    if p >= 1. {
        return draw(g, color);
    }
    let s = px(g.size());
    if p <= 0. {
        return div().flex_none().size(s).into_any_element();
    }
    if g.sprite().is_some() {
        let k = ((p * CUTS as f32).ceil() as u32).clamp(1, CUTS);
        let path = cut_path(g, k).unwrap_or_default();
        return svg().path(SharedString::new_static(path)).flex_none().size(s).text_color(color).into_any_element();
    }
    let mut el = div().relative().flex_none().size(s);
    for &part in g.parts() {
        el = match part {
            Part::Fill(x, y, w, h) => el.child(div().absolute().left(px(x)).top(px(y)).w(px(w * p)).h(px(h)).bg(color)),
            Part::Frame(x, y, w, h) => {
                el.child(div().absolute().left(px(x)).top(px(y)).w(px(w * p)).h(px(h)).border_1().border_color(color))
            }
            Part::Line(_) | Part::Poly(_) => el,
        };
    }
    el.into_any_element()
}

/// The TUI's running mark (agent-tui `app.rs:399` `SPINNER_FRAMES`):
/// a 3–4 dot snake round the six top dots of a braille cell.
pub(crate) const SPINNER_FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// Braille dots 1–6 in the 10 box (polish.md P1): columns at x 2 (dots
/// 1 2 3) and 6 (4 5 6), rows at y 1, 4 and 7; each a 2 px square.
pub(crate) const DOTS: [(f32, f32); 6] = [(2., 1.), (2., 4.), (2., 7.), (6., 1.), (6., 4.), (6., 7.)];
pub(crate) const DOT: f32 = 2.;

/// An unlit dot: `UNLIT` of the way from the ground to the mark, so the
/// 2×3 silhouette always reads as one glyph, by shape.
pub(crate) const UNLIT: f32 = 0.2;

/// A braille character's dots 1–6 (bit n−1: dot n), as Unicode numbers them.
pub(crate) const fn dots_of(c: char) -> u8 {
    ((c as u32 - 0x2800) & 0x3f) as u8
}

/// The running snake, one frame a beat: [`SPINNER_FRAMES`] decoded.
pub(crate) const RUN_FRAMES: [u8; 10] = {
    let mut out = [0; 10];
    let mut i = 0;
    while i < 10 {
        out[i] = dots_of(SPINNER_FRAMES[i]);
        i += 1;
    }
    out
};

/// Workers running in an idle session: two opposite dots, {1,6} → {4,3} →
/// {5,2}, one step every two beats.
pub(crate) const WORKERS_FRAMES: [u8; 3] = [0b10_0001, 0b00_1100, 0b01_0010];

/// Opening: one dot round the ring (1 → 4 → 5 → 6 → 3 → 2), one step
/// every two beats: a dial.
pub(crate) const OPENING_FRAMES: [u8; 6] = [0b00_0001, 0b00_1000, 0b01_0000, 0b10_0000, 0b00_0100, 0b00_0010];

/// The still frame (Reduced, Off, low power, history): `Run` ⠹ (dots
/// 1 4 5 6); `Workers` and `Opening` their first frames (two dots, one).
pub(crate) const STILL: u8 = dots_of('⠹');

impl Glyph {
    /// One of the braille family (polish.md P1).
    pub(crate) fn spins(self) -> bool {
        matches!(self, Glyph::Run | Glyph::Workers | Glyph::Opening)
    }

    /// Beats a frame of its spin lasts: the snake 1, the two heads and the
    /// dial 2.
    pub(crate) fn spin_every(self) -> u32 {
        if self == Glyph::Run { 1 } else { 2 }
    }
}

/// A braille glyph's lit dots at `beat` (phase-locked app-wide: every
/// running thing spins in step); `None`: its still frame.
pub(crate) fn spin_dots(g: Glyph, beat: Option<u32>) -> u8 {
    match (g, beat) {
        (Glyph::Run, Some(b)) => RUN_FRAMES[(b % 10) as usize],
        (Glyph::Run, None) => STILL,
        (Glyph::Workers, b) => WORKERS_FRAMES[(b.unwrap_or(0) / 2 % 3) as usize],
        (_, b) => OPENING_FRAMES[(b.unwrap_or(0) / 2 % 6) as usize],
    }
}

/// Paints a braille frame (`dots` lit) in the box at `origin`: six quads,
/// origins and sizes on device pixels (a 2 px dot stays one solid square at
/// 1.25× or 1.5×). The app's frame and the beat layer paint the same.
pub(crate) fn paint_dots(window: &mut Window, origin: Point<Pixels>, dots: u8, mark: Hsla, ground: Hsla) {
    let s = window.scale_factor();
    let snap = |v: f32| (v * s).round() / s;
    let d = snap(DOT).max(1. / s);
    let unlit = crate::motion::mix(ground, mark, UNLIT);
    let (ox, oy) = (f32::from(origin.x), f32::from(origin.y));
    for (i, &(x, y)) in DOTS.iter().enumerate() {
        let c = if dots & (1 << i) != 0 { mark } else { unlit };
        window.paint_quad(fill(Bounds::new(point(px(snap(ox + x)), px(snap(oy + y))), size(px(d), px(d))), c));
    }
}

/// A braille glyph (polish.md P1) at `beat` (`None`: still), in `mark`
/// on `ground`: its six dots, painted on device pixels.
pub(crate) fn spin(g: Glyph, beat: Option<u32>, mark: Hsla, ground: Hsla) -> Div {
    let dots = spin_dots(g, beat);
    div()
        .relative()
        .flex_none()
        .size(px(10.))
        .debug_selector(move || format!("spin-{dots:02x}"))
        .child(canvas(|_, _, _| {}, move |b, _, window, _| paint_dots(window, b.origin, dots, mark, ground)).absolute().inset_0())
}

/// `Needs`' inner cell in its box: x, y, side.
pub(crate) const BREATH_CELL: (f32, f32, f32) = (3., 3., 4.);

/// The breathing cell's colour `step` beats in: between `mark` and
/// `mix(mark, ground, 0.6)`, 19 beats a cycle.
pub(crate) fn breath_cell(step: u32, mark: Hsla, ground: Hsla) -> Hsla {
    let n = crate::motion::BREATH_BEATS;
    let f = 1. - ((2 * (step % n)) as f32 / n as f32 - 1.).abs();
    crate::motion::mix(mark, ground, 0.6 * f)
}

/// `Needs` breathing (motion.md §6.3): `step` beats since the prompt came,
/// the inner cell between `mark` and `mix(mark, ground, 0.6)`, 19 beats a
/// cycle, three cycles, then held at `mark`. `None`: still.
pub(crate) fn breath(step: Option<u32>, mark: Hsla, ground: Hsla) -> AnyElement {
    let Some(s) = step.filter(|s| *s < crate::motion::BREATH_BEATS * crate::motion::BREATHS) else {
        return draw(Glyph::Needs, mark);
    };
    breath_box(breath_cell(s, mark, ground), mark).into_any_element()
}

fn breath_box(cell: Hsla, mark: Hsla) -> Div {
    let (x, y, w) = BREATH_CELL;
    div()
        .relative()
        .flex_none()
        .size(px(10.))
        .child(div().absolute().inset_0().border_1().border_color(mark))
        .child(div().absolute().left(px(x)).top(px(y)).size(px(w)).bg(cell))
}

/// A glyph whose beat the beat layer paints ([`crate::motion::Beat`]):
/// drawn still (a spinner's still frame; a breath's cell at `mark`, where
/// it ends), and where it was painted recorded for the layer. Pixel for
/// pixel what [`spin`] and [`breath`] draw, the layer's dots or cell
/// painted over it.
pub(crate) fn on_beat(beat: crate::motion::Beat, mark: Hsla, ground: Hsla, fx: &std::rc::Rc<crate::motion::Motion>) -> AnyElement {
    let el = match beat {
        crate::motion::Beat::Spin(g) => spin(g, None, mark, ground),
        crate::motion::Beat::Breath(_) => breath_box(mark, mark),
        crate::motion::Beat::Orbit => div(),
    };
    let fx = fx.clone();
    el.child(
        canvas(
            |_, _, _| {},
            move |b, _, window, _| fx.site(crate::motion::Site { origin: b.origin, mask: window.content_mask(), mark, ground, beat }),
        )
        .absolute()
        .inset_0(),
    )
    .into_any_element()
}

/// A state's glyph changing (motion.md §6.1, §6.2) at `p` (0..1): to `Ok`
/// or `Failed` its strokes write on; to `Cancelled`, `Limit` or
/// `Cancelling` (not from `Cancelling`) its quads draw; else (or `fade`)
/// the old glyph's colour goes to the ground as the new one's comes from
/// it: a cross-fade by colour, as an opacity would paint it.
pub(crate) fn change(from: Option<(Glyph, Hsla)>, to: Glyph, mark: Hsla, ground: Hsla, p: f32, fade: bool) -> AnyElement {
    if p >= 1. {
        return draw_on(to, mark, ground);
    }
    if draws(from.map(|f| f.0), to) && !fade {
        return draw_at(to, p, mark);
    }
    let s = px(to.size().max(from.map_or(0., |(g, _)| g.size())));
    div()
        .relative()
        .flex_none()
        .size(s)
        .children(
            from.filter(|(g, _)| *g != to)
                .map(|(g, c)| div().absolute().inset_0().child(draw_on(g, crate::motion::mix(c, ground, p), ground))),
        )
        .child(div().absolute().inset_0().child(draw_on(to, crate::motion::mix(ground, mark, p), ground)))
        .into_any_element()
}

/// A change to `to` from `from` draws its strokes (or quads) on.
fn draws(from: Option<Glyph>, to: Glyph) -> bool {
    match to {
        Glyph::Ok | Glyph::Failed => true,
        Glyph::Cancelled | Glyph::Limit | Glyph::Cancelling => from != Some(Glyph::Cancelling),
        _ => false,
    }
}

/// [`change`] without the fade (polish.md P4, finish gate #17): to a mark
/// that draws, its strokes write on (`p`) over the glyph it replaces, which
/// goes to the ground meanwhile (`old`, 1 → 0; a spinner in the frame it
/// showed then, `beat`). The box is never empty. Else [`change`]'s
/// cross-fade, on `p`.
pub(crate) fn change_over(from: Option<(Glyph, Option<u32>, Hsla)>, to: Glyph, mark: Hsla, ground: Hsla, old: f32, p: f32) -> AnyElement {
    if !draws(from.map(|f| f.0), to) || (p >= 1. && old <= 0.) {
        return change(from.map(|(g, _, c)| (g, c)), to, mark, ground, p, false);
    }
    let s = px(to.size().max(from.map_or(0., |(g, ..)| g.size())));
    let centred = |el: AnyElement| div().absolute().inset_0().flex().items_center().justify_center().child(el);
    div()
        .relative()
        .flex_none()
        .size(s)
        .debug_selector(move || format!("write-on {old:.2} {p:.2}"))
        .children(from.filter(|(g, ..)| old > 0. && *g != to).map(|(g, beat, c)| {
            let c = crate::motion::mix(ground, c, old);
            centred(if g.spins() { spin(g, beat, c, ground).into_any_element() } else { draw(g, c) })
        }))
        .children((p > 0.).then(|| centred(draw_at(to, p, mark))))
        .into_any_element()
}

/// The fold chevron at `p` of its turn (0 `Fold` … 1 `FoldOpen`): its
/// points rotated about the box centre (the sprite drawn turned).
pub(crate) fn turn(p: f32, color: Hsla) -> AnyElement {
    if p <= 0. {
        return draw(Glyph::Fold, color);
    }
    if p >= 1. {
        return draw(Glyph::FoldOpen, color);
    }
    let path = Glyph::Fold.sprite().unwrap_or_default();
    svg()
        .path(path)
        .flex_none()
        .size(px(10.))
        .text_color(color)
        .with_transformation(Transformation::rotate(radians(p * std::f32::consts::FRAC_PI_2)))
        .into_any_element()
}

/// The app's assets: the stroked glyphs' SVGs, embedded (written from the
/// drawing table, so the two never disagree). Loaded only when the atlas
/// has no sprite of that size and scale yet.
pub(crate) struct Sprites;

impl AssetSource for Sprites {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        if path.starts_with("life/") {
            return Ok(life_svg(path).map(|s| Cow::Owned(s.into_bytes())));
        }
        Ok(parse_cut(path).map(|(g, k)| Cow::Owned(svg_cut(g, k).into_bytes())))
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(Glyph::ALL.into_iter().filter_map(Glyph::sprite).filter(|p| p.starts_with(path)).map(SharedString::from).collect())
    }
}

/// A row's lead column (design.md §3): `m.lead` wide (12·r and the gap of
/// 8·r), its glyph centred in the first part, or nothing: the column stays,
/// so nothing shifts.
pub(crate) fn lead(g: Option<AnyElement>, m: &Metrics) -> Div {
    let col = m.lead - (8. * m.r).round();
    div().flex_none().w(px(m.lead)).flex().items_center().child(div().flex().flex_none().w(px(col)).justify_center().children(g))
}

/// The house mark (design.md §2): 2 px on its container's left edge, its
/// full height. An absolute child (the container is `relative`): it never
/// moves content.
pub(crate) fn bar(color: Hsla) -> Div {
    div().absolute().left_0().top_0().bottom_0().w(px(2.)).bg(color)
}

/// [`bar`] drawn to `p` of its length, top down (motion.md §6.1: a bar
/// draws; an un-draw is the same bar going back up). Paint only: an
/// absolute child's length.
pub(crate) fn bar_at(color: Hsla, p: f32) -> Div {
    if p >= 1. { bar(color) } else { div().absolute().left_0().top_0().w(px(2.)).h(relative(p.max(0.))).bg(color) }
}

/// A reveal button's glyph (design.md §5): its slot always kept, drawn
/// while `group` (its row) is hovered or `shown` (the row is selected), in
/// `rest`; in `on` while the pointer is on the button itself (`own`, the
/// button's group). A stroked glyph (a sprite): one element.
pub(crate) fn reveal(g: Glyph, group: &'static str, own: &'static str, shown: bool, rest: Hsla, on: Hsla) -> Div {
    let s = px(g.size());
    let mark = match g.sprite() {
        Some(path) => {
            svg().path(path).flex_none().size(s).text_color(rest).group_hover(own, move |st| st.text_color(on)).into_any_element()
        }
        None => draw(g, rest),
    };
    div().flex_none().size(s).when(!shown, |d| d.invisible().group_hover(group, |d| d.visible())).child(mark)
}

/// What a glyph and its word are painted in (design.md §4.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Family {
    Run,
    Warn,
    Err,
    Ok,
    Rest,
    Unread,
    /// Opening: the one state that is yours (the accent).
    Open,
    /// The composer foot's READY.
    Ready,
}

/// Where a glyph sits (design.md §4.4). `Run`: a run's surfaces (`tool`,
/// `tool_out`): its rows, worker blocks and the tray. `User`: the composer
/// foot. `Chrome`: the rail, the tabs, the top bar. `Card`: the NEEDS YOU
/// stub, the authorization card and the overlays (the hover card).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ground {
    Canvas,
    Run,
    User,
    Chrome,
    Card,
}

/// A family's (text, mark) roles on a ground: the only place a state
/// becomes a colour. A family the table has no column for on a ground
/// (unread off the chrome; opening and ready off theirs) takes rest's.
pub(crate) fn ink(f: Family, g: Ground, p: &Palette) -> (Hsla, Hsla) {
    use Family as F;
    use Ground as G;
    match (g, f) {
        (G::Canvas, F::Run) => (p.run, p.run_mark),
        (G::Canvas, F::Warn) => (p.warn, p.warn_mark),
        (G::Canvas, F::Err) => (p.err, p.err),
        (G::Canvas, F::Ok) => (p.ok, p.ok),
        (G::Canvas, F::Ready) => (p.ready, p.ready),
        (G::Canvas, _) => (p.dim, p.dim),
        (G::Run, F::Run) => (p.run, p.run_mark),
        (G::Run, F::Warn) => (p.warn, p.warn_mark),
        (G::Run, F::Err) => (p.err, p.err),
        (G::Run, F::Ok) => (p.ok, p.ok),
        (G::Run, _) => (p.dim_t, p.dim_t),
        (G::User, F::Run) => (p.run, p.run_mark),
        (G::User, F::Warn) => (p.warn, p.warn_mark),
        (G::User, F::Err) => (p.err, p.err),
        (G::User, F::Ok) => (p.ok, p.ok),
        (G::User, F::Ready) => (p.ready, p.ready),
        // The word is the hint's ("opening…"); `you` is not floored on `user`.
        (G::User, F::Open) => (p.dim_u, p.you_mark),
        (G::User, _) => (p.dim_u, p.dim_u),
        (G::Chrome, F::Run) => (p.run_ch, p.run_mark_ch),
        (G::Chrome, F::Warn) => (p.warn_ch, p.warn_mark_ch),
        (G::Chrome, F::Err) => (p.err_ch, p.err_mark_ch),
        (G::Chrome, F::Ok) => (p.ok_ch, p.ok_ch),
        (G::Chrome, F::Rest) => (p.dim_ch, p.rest_mark),
        (G::Chrome, F::Unread) => (p.unread_ch, p.unread_mark_ch),
        (G::Chrome, F::Open) => (p.open_ch, p.focus_ch),
        (G::Chrome, F::Ready) => (p.ready_ch, p.ready_mark_ch),
        (G::Card, F::Run) => (p.run_k, p.run_mark_k),
        (G::Card, F::Warn) => (p.warn_k, p.warn_mark),
        (G::Card, F::Err) => (p.err_k, p.err_mark_k),
        (G::Card, F::Ok) => (p.ok_k, p.ok_k),
        (G::Card, _) => (p.dim_k, p.rest_mark_k),
    }
}

/// A session's state (the rail's [`State`]): its glyph (none when saved)
/// and its family (design.md §4.2). Its word is [`State::word`]; the
/// composer foot says READY for idle ([`foot`]).
pub(crate) fn session(st: State) -> (Option<Glyph>, Family) {
    match st {
        State::NeedsYou => (Some(Glyph::Needs), Family::Warn),
        State::Running => (Some(Glyph::Run), Family::Run),
        State::Workers => (Some(Glyph::Workers), Family::Run),
        State::Unread => (Some(Glyph::Unread), Family::Unread),
        State::Idle => (Some(Glyph::Idle), Family::Rest),
        State::Parked => (Some(Glyph::Parked), Family::Rest),
        State::Opening => (Some(Glyph::Opening), Family::Open),
        State::Locked => (Some(Glyph::Locked), Family::Rest),
        State::Saved => (None, Family::Rest),
    }
}

/// A session's glyph on `ground`, in a 10 px box (empty when it has none),
/// still.
pub(crate) fn session_mark(st: State, ground: Ground, p: &Palette) -> Div {
    let (g, f) = session(st);
    let (_, mark) = ink(f, ground, p);
    div().flex().flex_none().size(px(10.)).children(g.map(|g| draw_on(g, mark, ground.color(p))))
}

/// [`session_mark`] live, on `bg` (the row's or tab's fill): a running,
/// workers or opening glyph spins on the beat (polish.md P1).
pub(crate) fn session_live(st: State, ground: Ground, bg: Hsla, p: &Palette, fx: &std::rc::Rc<crate::motion::Motion>) -> Div {
    let (g, f) = session(st);
    let (_, mark) = ink(f, ground, p);
    div().flex().flex_none().size(px(10.)).children(g.map(|g| spinning(fx, g, mark, bg)))
}

/// `g` as the beat has it: a braille glyph spins (the beat layer paints it
/// over its still frame; without one, the app's frame draws the beat's);
/// any other is drawn.
pub(crate) fn spinning(fx: &std::rc::Rc<crate::motion::Motion>, g: Glyph, mark: Hsla, ground: Hsla) -> AnyElement {
    if !g.spins() {
        return draw(g, mark);
    }
    if fx.layered() {
        return on_beat(crate::motion::Beat::Spin(g), mark, ground, fx);
    }
    spin(g, fx.beat(g.spin_every()), mark, ground).into_any_element()
}

impl Ground {
    /// The ground's own colour.
    pub(crate) fn color(self, p: &Palette) -> Hsla {
        match self {
            Ground::Canvas | Ground::Run => p.canvas,
            Ground::User => p.user,
            Ground::Chrome => p.chrome,
            Ground::Card => p.card,
        }
    }
}

/// A session's word colour on `ground` (its state word and counts).
pub(crate) fn session_word(st: State, ground: Ground, p: &Palette) -> Hsla {
    ink(session(st).1, ground, p).0
}

/// The composer foot's state (design.md §8.11): the session's glyph, its
/// word (READY when idle; none while opening: the hint says it) and its
/// family, on the user ground.
pub(crate) fn foot(st: State) -> (Option<Glyph>, Option<&'static str>, Family) {
    match st {
        State::Idle => (Some(Glyph::Idle), Some(label::READY), Family::Ready),
        State::Opening => (Some(Glyph::Opening), None, Family::Open),
        _ => {
            let (g, f) = session(st);
            (g, st.word(), f)
        }
    }
}

/// A call's outcome (design.md §8.1): its glyph, its word (none when it
/// ended ok: the commonest outcome stays silent) and its family. `waiting`:
/// a prompt waits on it; `seen`: not attached, so a call that has not
/// ended is as last seen.
pub(crate) fn call(st: Option<ToolStatus>, waiting: bool, seen: bool) -> (Glyph, Option<&'static str>, Family) {
    match st {
        Some(ToolStatus::Done) => (Glyph::Ok, None, Family::Ok),
        // Cards D (cards4/proposal.md §2, §4): sentence-case Sans words; a
        // failed bash says its exit code instead (`turns::exit_code`).
        Some(ToolStatus::Failed) => (Glyph::Failed, Some("Failed"), Family::Err),
        Some(ToolStatus::Denied) => (Glyph::Failed, Some("Denied"), Family::Err),
        Some(ToolStatus::Cancelled) => (Glyph::Cancelled, Some("Cancelled"), Family::Warn),
        // The daemon stopped it at its output limit: neither ok nor failed;
        // its facts say so.
        Some(ToolStatus::Unknown) => (Glyph::Limit, Some("stopped at output limit"), Family::Warn),
        Some(ToolStatus::Queued) => (Glyph::Idle, Some("Queued"), Family::Rest),
        _ if waiting => (Glyph::Needs, Some("Waiting for you"), Family::Warn),
        _ if seen => (Glyph::Idle, Some("Last seen running"), Family::Rest),
        // Running says nothing: its orbit and its clock do.
        _ => (Glyph::Run, None, Family::Run),
    }
}

/// A worker's state as motion keys it: a re-attach's demoted start
/// (`demoted`, [`crate::subagents::View::demoted`]) is still running, so
/// neither the reconnect's demote nor its lift on the next page plays a
/// write-on (motion.md §8: a reconnect is history).
pub(crate) fn steady(l: Liveness, demoted: bool) -> Liveness {
    if demoted && l == Liveness::Started { Liveness::Running } else { l }
}

/// A worker's glyph, its word outside run surfaces (Display caps) and
/// inside them (Mono), and its family (design.md §4.2).
pub(crate) fn worker(l: Liveness) -> (Glyph, &'static str, &'static str, Family) {
    match l {
        Liveness::Started => (Glyph::Idle, label::STARTED, "started", Family::Rest),
        Liveness::Running => (Glyph::Run, label::RUNNING, "running", Family::Run),
        Liveness::Cancelling => (Glyph::Cancelling, label::CANCELLING, "cancelling", Family::Warn),
        Liveness::Done => (Glyph::Ok, label::COMPLETED, "completed", Family::Ok),
        Liveness::Failed => (Glyph::Failed, label::FAILED, "failed", Family::Err),
        Liveness::Cancelled => (Glyph::Cancelled, label::CANCELLED, "cancelled", Family::Warn),
        Liveness::TimedOut => (Glyph::Limit, label::TIMED_OUT, "timed out", Family::Warn),
        Liveness::LastSeen => (Glyph::Idle, label::LAST_SEEN, "last seen running", Family::Rest),
        Liveness::Gone => (Glyph::Gone, label::GONE, "gone", Family::Rest),
    }
}

/// A folded run's stacked tally bar (design.md §8.2): its tools, most used
/// first (`n` of each: its calls), in a bar `h` px tall. The first
/// `K = min(tools, h/4)` are drawn (the proportional part keeps at least
/// half the bar); each gets 2 px and its share of the rest,
/// `(h − 2K)·nᵢ / Σ_K n`, floored. The pixels flooring leaves go one each
/// to the largest fractional parts, ties in tally order (Hamilton's
/// method): every segment is within 1 px of its exact share, equal counts
/// within 1 px of each other, heights never grow down the tally, and the
/// tools not drawn take no one's colour. Yields (index, top, height), top
/// to bottom; nothing allocated (K ≤ h/4: a rank is K steps).
// Cards D folds a run without its bar (C5 bent); kept, with its tests,
// for the rail pass.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn tally_bar<'a, T>(tally: &'a [T], n: impl Fn(&T) -> usize + 'a, h: u32) -> impl Iterator<Item = (usize, u32, u32)> + 'a {
    let k = tally.len().min(h as usize / 4);
    let drawn = &tally[..k];
    // A tool in a tally was called at least once.
    let n = move |t: &T| n(t).max(1);
    let sum = drawn.iter().map(&n).sum::<usize>().max(1);
    let free = h as usize - 2 * k;
    // Its whole pixels and what flooring cut off (in 1/sum px).
    let share = move |t: &T| (free * n(t) / sum, free * n(t) % sum);
    let left = free - drawn.iter().map(|t| share(t).0).sum::<usize>();
    let mut top = 0;
    drawn.iter().enumerate().map(move |(i, t)| {
        let (whole, cut) = share(t);
        let ahead = drawn.iter().enumerate().filter(|&(j, u)| {
            let c = share(u).1;
            c > cut || (c == cut && j < i)
        });
        let s = 2 + (whole + usize::from(ahead.count() < left)) as u32;
        let seg = (i, top, s);
        top += s;
        seg
    })
}

// ── Cards D's life glyphs (cards4/proposal.md §4, §9) ─────────────────────

/// A call's or a worker's state in a card (cards4/proposal.md §4): one
/// 16 px stroked family, 1.5 px strokes, each its own sprite (rasterized
/// once per size and scale, batched with the text). The rail and the tabs
/// keep the braille family above.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Life {
    /// A dotted ring, `dim`.
    Queued,
    /// The orbit: an arc lapping a faint ring, `acc` ([`orbit`]).
    Orbit,
    /// Not attached: as last seen, the arc held still in `dim`.
    Seen,
    /// A tick, `dim` (lit `ok` as it lands live).
    Done,
    /// A cross on a 35 % `err` ring.
    Failed,
    /// A dash on a `dim` ring: stopped at the output limit.
    Limit,
    /// An `acc` dot in an `acc` ring at 55 %.
    Wait,
    /// A ring and a slash, `err`.
    Denied,
    /// A dash on a 45 % `dim` ring.
    Cancelled,
}

impl Life {
    pub(crate) const ALL: [Life; 9] =
        [Life::Queued, Life::Orbit, Life::Seen, Life::Done, Life::Failed, Life::Limit, Life::Wait, Life::Denied, Life::Cancelled];

    /// Its index: what [`crate::motion::Motion::change`] remembers.
    pub(crate) fn code(self) -> u64 {
        Life::ALL.iter().position(|l| *l == self).unwrap_or(0) as u64
    }

    pub(crate) fn of_code(c: u64) -> Option<Life> {
        Life::ALL.get(c as usize).copied()
    }

    /// Its mark's strokes (the part that writes on), and its ring.
    fn mark(self) -> Option<Mark> {
        Some(match self {
            Life::Done => Mark::Tick,
            Life::Failed => Mark::Cross,
            Life::Denied => Mark::Slash,
            Life::Limit | Life::Cancelled => Mark::Dash,
            Life::Wait => Mark::Dot,
            Life::Queued | Life::Orbit | Life::Seen => return None,
        })
    }
}

/// The box every life glyph is drawn in.
pub(crate) const LIFE: f32 = 16.;
/// Its ring's radius and stroke.
const LIFE_R: f32 = 6.;
const LIFE_STROKE: f32 = 1.5;

/// A life glyph's mark (its strokes cut for the write-on, as [`CUTS`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mark {
    Tick,
    Cross,
    Slash,
    Dash,
    Dot,
}

impl Mark {
    const ALL: [Mark; 5] = [Mark::Tick, Mark::Cross, Mark::Slash, Mark::Dash, Mark::Dot];

    fn stem(self) -> &'static str {
        match self {
            Mark::Tick => "life/tick",
            Mark::Cross => "life/cross",
            Mark::Slash => "life/slash",
            Mark::Dash => "life/dash",
            Mark::Dot => "life/dot",
        }
    }

    fn lines(self) -> &'static [&'static [(f32, f32)]] {
        match self {
            Mark::Tick => &[&[(4.5, 8.3), (7., 10.8), (11.5, 5.4)]],
            Mark::Cross => &[&[(5.6, 5.6), (10.4, 10.4)], &[(10.4, 5.6), (5.6, 10.4)]],
            // Ring to ring at 45°.
            Mark::Slash => &[&[(3.76, 12.24), (12.24, 3.76)]],
            Mark::Dash => &[&[(5., 8.), (11., 8.)]],
            Mark::Dot => &[],
        }
    }
}

/// The sprite paths, built once: `life/ring.svg`, `life/dots.svg`, the
/// arc at each whole percent of the turn (`life/arc~N.svg`, 1..=100), each
/// mark at each cut (`life/tick~k.svg`, `k` 1..=CUTS), and the ping's ring
/// at each half pixel of its radius (`life/ping~R.svg`, `R` 12..=22, in a
/// 24 px box).
struct LifePaths {
    arcs: Vec<SharedString>,
    marks: Vec<Vec<SharedString>>,
    pings: Vec<SharedString>,
}

static LIFE_PATHS: std::sync::LazyLock<LifePaths> = std::sync::LazyLock::new(|| LifePaths {
    arcs: (1..=100).map(|n| SharedString::from(format!("life/arc~{n}.svg"))).collect(),
    marks: Mark::ALL.iter().map(|m| (1..=CUTS).map(|k| SharedString::from(format!("{}~{k}.svg", m.stem()))).collect()).collect(),
    pings: (PING_FROM..=PING_TO).map(|r| SharedString::from(format!("life/ping~{r}.svg"))).collect(),
});

/// The ping's ring, in half pixels of its radius: 6 → 11 (§8 #7).
const PING_FROM: u32 = 12;
const PING_TO: u32 = 22;
const PING_BOX: f32 = 24.;

const RING: &str = "life/ring.svg";
const DOTTED: &str = "life/dots.svg";

/// The arc `len` (turns) long: its sprite, at a whole percent (none under 1 %).
fn arc_path(len: f32) -> Option<SharedString> {
    let n = (len * 100.).round().clamp(0., 100.) as usize;
    (n > 0).then(|| LIFE_PATHS.arcs[n - 1].clone())
}

fn mark_path(m: Mark, k: u32) -> Option<SharedString> {
    let i = Mark::ALL.iter().position(|x| *x == m)?;
    (k > 0).then(|| LIFE_PATHS.marks[i][(k.min(CUTS) - 1) as usize].clone())
}

/// A life sprite's SVG, written from the table above.
fn life_svg(path: &str) -> Option<String> {
    let (s, w) = (LIFE, LIFE_STROKE);
    let head =
        |size: f32| format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="{size}" height="{size}" viewBox="0 0 {size} {size}">"#);
    let c = LIFE / 2.;
    let body = match path {
        RING => format!(r##"<circle cx="{c}" cy="{c}" r="{LIFE_R}" fill="none" stroke="#000" stroke-width="{w}"/>"##),
        DOTTED => format!(
            r##"<circle cx="{c}" cy="{c}" r="{LIFE_R}" fill="none" stroke="#000" stroke-width="{w}" stroke-dasharray="1.4 1.74"/>"##
        ),
        _ => {
            let (stem, k) = path.strip_suffix(".svg")?.rsplit_once('~')?;
            let k: u32 = k.parse().ok()?;
            match stem {
                "life/arc" if (1..=100).contains(&k) => {
                    if k == 100 {
                        format!(r##"<circle cx="{c}" cy="{c}" r="{LIFE_R}" fill="none" stroke="#000" stroke-width="{w}"/>"##)
                    } else {
                        let a = k as f32 / 100. * std::f32::consts::TAU;
                        let (x, y) = (c + LIFE_R * a.sin(), c - LIFE_R * a.cos());
                        let large = u8::from(k > 50);
                        format!(
                            r##"<path d="M{c} {} A{LIFE_R} {LIFE_R} 0 {large} 1 {:.3} {:.3}" fill="none" stroke="#000" stroke-width="{w}" stroke-linecap="round"/>"##,
                            c - LIFE_R,
                            x,
                            y
                        )
                    }
                }
                "life/ping" if (PING_FROM..=PING_TO).contains(&k) => {
                    let c = PING_BOX / 2.;
                    return Some(format!(
                        r##"{}<circle cx="{c}" cy="{c}" r="{}" fill="none" stroke="#000" stroke-width="{w}"/></svg>"##,
                        head(PING_BOX),
                        k as f32 / 2.
                    ));
                }
                _ => {
                    let m = Mark::ALL.into_iter().find(|m| m.stem() == stem)?;
                    if !(1..=CUTS).contains(&k) {
                        return None;
                    }
                    if m == Mark::Dot {
                        format!(r##"<circle cx="{c}" cy="{c}" r="{}" fill="#000"/>"##, 2.5 * k as f32 / CUTS as f32)
                    } else {
                        format!(
                            r##"<path d="{}" fill="none" stroke="#000" stroke-width="{w}" stroke-linecap="round" stroke-linejoin="round"/>"##,
                            cut_lines(m.lines(), k)
                        )
                    }
                }
            }
        }
    };
    Some(format!("{}{body}</svg>", head(s)))
}

/// Polylines drawn to `k / CUTS` of their whole length, in order: an SVG path.
fn cut_lines(lines: &[&[(f32, f32)]], k: u32) -> String {
    let seg = |a: (f32, f32), b: (f32, f32)| ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
    let total: f32 = lines.iter().map(|pts| pts.windows(2).map(|w| seg(w[0], w[1])).sum::<f32>()).sum();
    let mut left = total * k.min(CUTS) as f32 / CUTS as f32;
    let mut d = String::new();
    for pts in lines {
        if left <= 1e-4 {
            break;
        }
        let _ = write!(d, "M{} {}", pts[0].0, pts[0].1);
        for w in pts.windows(2) {
            let l = seg(w[0], w[1]);
            if left >= l - 1e-4 {
                let _ = write!(d, "L{} {}", w[1].0, w[1].1);
                left -= l;
            } else {
                let t = left / l;
                let _ = write!(d, "L{:.2} {:.2}", w[0].0 + (w[1].0 - w[0].0) * t, w[0].1 + (w[1].1 - w[0].1) * t);
                left = 0.;
                break;
            }
        }
    }
    d
}

/// A sprite filling the 16 px box, in `color`.
fn sprite16(path: impl Into<SharedString>, color: Hsla) -> Svg {
    svg().path(path).absolute().top_0().left_0().size(px(LIFE)).text_color(color)
}

/// What a life glyph is painted in (§4), on `ground` (the row's fill):
/// its ring (none for queued, done and the orbit's own track) and its mark.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LifeInk {
    pub(crate) ring: Option<Hsla>,
    pub(crate) mark: Hsla,
}

/// A life glyph's colours (§4): `dim`, `err` and `acc` on the card's ground.
pub(crate) fn life_ink(l: Life, ground: Hsla, p: &Palette) -> LifeInk {
    use crate::motion::mix;
    let (ring, mark) = match l {
        Life::Queued | Life::Seen => (None, p.dim_t),
        Life::Orbit => (Some(mix(ground, p.dim_t, 0.28)), p.acc),
        Life::Done => (None, p.dim_t),
        Life::Failed => (Some(mix(ground, p.err, 0.35)), p.err),
        Life::Limit => (Some(p.dim_t), p.dim_t),
        Life::Wait => (Some(mix(ground, p.acc, 0.55)), p.acc),
        Life::Denied => (Some(p.err), p.err),
        Life::Cancelled => (Some(mix(ground, p.dim_t, 0.45)), p.dim_t),
    };
    LifeInk { ring, mark }
}

/// A life glyph at rest (§4): its ring and its whole mark, or the dotted
/// ring, or the arc held still ([`crate::motion::ORBIT_STILL`]).
pub(crate) fn life(l: Life, ink: LifeInk) -> Div {
    life_at(l, ink, 1., 1.)
}

/// A life glyph with its ring at `ring` (0..1 of its colour, from the
/// ground: the caller mixes) and its mark drawn to `p` (the write-on: 0..1
/// of its strokes; the dot grows).
pub(crate) fn life_at(l: Life, ink: LifeInk, ring: f32, p: f32) -> Div {
    let el = div().relative().flex_none().size(px(LIFE)).debug_selector(move || format!("life-{l:?}"));
    match l {
        Life::Queued => el.child(sprite16(DOTTED, ink.mark)),
        Life::Orbit | Life::Seen => {
            el.children(ink.ring.map(|c| sprite16(RING, c))).children(arc_path(crate::motion::ORBIT_STILL).map(|a| sprite16(a, ink.mark)))
        }
        _ => {
            let k = ((p * CUTS as f32).ceil() as u32).min(CUTS);
            el.children(ink.ring.filter(|_| ring > 0.).map(|c| sprite16(RING, c)))
                .children(l.mark().and_then(|m| mark_path(m, k)).map(|m| sprite16(m, ink.mark)))
        }
    }
}

/// An arc `len` turns long, starting `start` turns from twelve o'clock: the
/// arc's sprite turned (no path: a GPUI path costs a full-window MSAA pass).
pub(crate) fn arc(start: f32, len: f32, color: Hsla) -> Option<Svg> {
    Some(sprite16(arc_path(len)?, color).with_transformation(Transformation::rotate(radians(start * std::f32::consts::TAU))))
}

/// The ring closing over the arc, `p` (0..1) of the way from `len`: the
/// success transition's first beat (§8 #5).
pub(crate) fn closing(start: f32, len: f32, p: f32, color: Hsla) -> Option<Svg> {
    arc(start * (1. - p), len + (1. - len) * p, color)
}

/// The running orbit (§4, §8 #2, #3): the faint track, and the arc lapping
/// it on the 30 Hz grid. `grow`: the arc's growth from nothing as the call
/// starts (0..1). With a beat layer and ambient on, the arc is the layer's
/// (the frame draws the track and records the site); without one, the
/// frame draws it (and asks for the next tick). Reduced, Off, low power:
/// the still arc, no frames.
pub(crate) fn orbit(fx: &std::rc::Rc<crate::motion::Motion>, ink: LifeInk, grow: f32) -> Div {
    #[cfg(test)]
    if fx.no_orbit.get() {
        // Tests of the 8 Hz beat alone: the rail's spinner in its place.
        return div().flex().flex_none().size(px(LIFE)).items_center().justify_center().child(spinning(fx, Glyph::Run, ink.mark, ink.mark));
    }
    let el =
        div().relative().flex_none().size(px(LIFE)).debug_selector(|| "life-Orbit".into()).children(ink.ring.map(|c| sprite16(RING, c)));
    if grow >= 1. && fx.layered() {
        let (fx, mark) = (fx.clone(), ink.mark);
        return el.child(
            canvas(
                |_, _, _| {},
                move |b, _, window, _| {
                    fx.site(crate::motion::Site {
                        origin: b.origin,
                        mask: window.content_mask(),
                        mark,
                        ground: mark,
                        beat: crate::motion::Beat::Orbit,
                    })
                },
            )
            .absolute()
            .inset_0(),
        );
    }
    let (start, len) = fx.orbit().unwrap_or((0., crate::motion::ORBIT_STILL));
    el.children(arc(start, len * grow.clamp(0., 1.), ink.mark))
}

/// The beat layer's orbit (§9): the arc's sprite at `origin`, turned.
pub(crate) fn paint_arc(window: &mut Window, cx: &App, origin: Point<Pixels>, (start, len): (f32, f32), color: Hsla) {
    let Some(path) = arc_path(len) else { return };
    let bounds = Bounds::new(origin, size(px(LIFE), px(LIFE)));
    let s = window.scale_factor();
    let c = bounds.center();
    let m = TransformationMatrix::unit().translate(c.scale(s)).rotate(radians(start * std::f32::consts::TAU)).translate(c.scale(-s));
    let _ = window.paint_svg(bounds, path, None, m, color, cx);
}

/// The ping (§8 #7, #8): a ring going out from the glyph's centre, `p`
/// (0..1) of the way, r 6 → 11, its colour `0.7 → 0` of the way off the
/// ground. Absolute, centred on the 16 px box (it overflows it).
pub(crate) fn ping(p: f32, color: Hsla, ground: Hsla) -> Svg {
    let r = (PING_FROM as f32 + (PING_TO - PING_FROM) as f32 * p).round() as u32;
    let path = LIFE_PATHS.pings[(r.clamp(PING_FROM, PING_TO) - PING_FROM) as usize].clone();
    let off = (LIFE - PING_BOX) / 2.;
    svg().path(path).absolute().left(px(off)).top(px(off)).size(px(PING_BOX)).text_color(crate::motion::mix(ground, color, 0.7 * (1. - p)))
}

/// A call's life glyph (§4) from its status: `waiting` on a prompt, `seen`
/// (not attached: a call not ended is as last seen).
pub(crate) fn life_of_call(st: Option<ToolStatus>, waiting: bool, seen: bool) -> Life {
    match st {
        Some(ToolStatus::Done) => Life::Done,
        Some(ToolStatus::Failed) => Life::Failed,
        Some(ToolStatus::Denied) => Life::Denied,
        Some(ToolStatus::Cancelled) => Life::Cancelled,
        Some(ToolStatus::Unknown) => Life::Limit,
        Some(ToolStatus::Queued) => Life::Queued,
        _ if waiting => Life::Wait,
        _ if seen => Life::Seen,
        _ => Life::Orbit,
    }
}

/// A worker's life glyph (§4, a spawn row and a chip).
pub(crate) fn life_of_worker(l: Liveness) -> Life {
    match l {
        Liveness::Started => Life::Queued,
        Liveness::Running => Life::Orbit,
        Liveness::Cancelling => Life::Orbit,
        Liveness::Done => Life::Done,
        Liveness::Failed => Life::Failed,
        Liveness::Cancelled => Life::Cancelled,
        Liveness::TimedOut => Life::Limit,
        Liveness::LastSeen | Liveness::Gone => Life::Seen,
    }
}

#[cfg(test)]
mod tests {
    use super::{Family, GO, Glyph, Part, Sprites, call, svg_of, tally_bar, worker};
    use crate::subagents::Liveness;
    use gpui_kit::AssetSource;
    use synaps_client::transcript::ToolStatus;

    /// P1: the running frames are the TUI's `SPINNER_FRAMES` decoded, each
    /// a 3–4 dot snake on the six top dots (the polish.md table, written
    /// out here so the decode itself is checked); the two heads and the
    /// dial keep their counts; one beat (or two) a frame, phase-locked.
    #[test]
    fn braille_frames_match_spinner_frames() {
        let table: [&[u8]; 10] = [
            &[1, 2, 4],
            &[1, 4, 5],
            &[1, 4, 5, 6],
            &[4, 5, 6],
            &[3, 4, 5, 6],
            &[3, 5, 6],
            &[2, 3, 6],
            &[1, 2, 3, 6],
            &[1, 2, 3],
            &[1, 2, 3, 4],
        ];
        let bits = |d: &[u8]| d.iter().fold(0u8, |b, n| b | 1 << (n - 1));
        for (i, lit) in table.iter().enumerate() {
            assert_eq!(super::RUN_FRAMES[i], bits(lit), "frame {i} ({})", super::SPINNER_FRAMES[i]);
            assert!((3..=4).contains(&super::RUN_FRAMES[i].count_ones()));
        }
        assert_eq!(super::STILL, bits(&[1, 4, 5, 6]), "⠹");
        for b in 0..40u32 {
            assert_eq!(super::spin_dots(Glyph::Run, Some(b)), super::RUN_FRAMES[(b % 10) as usize], "one frame a beat: a 1.25 s lap");
            assert_eq!(super::spin_dots(Glyph::Workers, Some(b)).count_ones(), 2, "two heads");
            assert_eq!(super::spin_dots(Glyph::Opening, Some(b)).count_ones(), 1, "a dial");
            assert_eq!(super::spin_dots(Glyph::Workers, Some(b)), super::spin_dots(Glyph::Workers, Some(b / 2 * 2)), "every two beats");
        }
        // The heads are opposite (1–6, 4–3, 5–2) and the dial walks the ring.
        assert_eq!(super::WORKERS_FRAMES, [bits(&[1, 6]), bits(&[4, 3]), bits(&[5, 2])]);
        assert_eq!(super::OPENING_FRAMES, [1u8, 4, 5, 6, 3, 2].map(|n| 1u8 << (n - 1)));
        // Still: the snake's ⠹, the heads' and the dial's first frames.
        assert_eq!(super::spin_dots(Glyph::Run, None), super::STILL);
        assert_eq!(super::spin_dots(Glyph::Workers, None), super::WORKERS_FRAMES[0]);
        assert_eq!(super::spin_dots(Glyph::Opening, None), super::OPENING_FRAMES[0]);
    }

    /// P1: a braille glyph is six 2×2 dots in the box (columns x 2 and 6,
    /// rows y 1, 4 and 7), its drawing table the still frame's lit ones.
    #[test]
    fn glyph_silhouette_is_six_dots() {
        assert_eq!(super::DOTS.len(), 6);
        for &(x, y) in &super::DOTS {
            assert!([2., 6.].contains(&x) && [1., 4., 7.].contains(&y) && x + super::DOT <= 10. && y + super::DOT <= 10.);
        }
        for g in [Glyph::Run, Glyph::Workers, Glyph::Opening] {
            assert!(g.spins());
            let lit: Vec<Part> = super::DOTS
                .iter()
                .enumerate()
                .filter(|(i, _)| super::spin_dots(g, None) & (1 << i) != 0)
                .map(|(_, &(x, y))| Part::Fill(x, y, super::DOT, super::DOT))
                .collect();
            assert_eq!(g.parts(), &lit[..], "{g:?}: its still frame");
        }
        assert_eq!(Glyph::ALL.iter().filter(|g| g.spins()).count(), 3);
    }

    /// Every cut's static path names its glyph and cut (Shady N7: the
    /// table stands in for a `format!` per write-on frame).
    #[test]
    fn every_cut_path_names_its_cut() {
        for g in Glyph::ALL.into_iter().filter(|g| g.sprite().is_some()) {
            for k in 1..=super::CUTS {
                let path = super::cut_path(g, k).expect("a sprite glyph's cut");
                assert_eq!(super::parse_cut(path), Some((g, k)), "{path}");
            }
        }
        assert_eq!(super::cut_path(Glyph::Run, 3), None, "quads are not cut");
    }

    /// The drawing table is injective (`GO` is `Fold` by design: one
    /// chevron) and every part stays inside its box, strokes
    /// with their half width.
    #[test]
    fn every_glyph_is_unique_and_inside_its_box() {
        for (i, a) in Glyph::ALL.iter().enumerate() {
            for b in &Glyph::ALL[i + 1..] {
                assert_ne!(a.parts(), b.parts(), "{a:?} and {b:?} are drawn alike");
            }
            let (s, half) = (a.size(), a.stroke() / 2.);
            for part in a.parts() {
                match *part {
                    Part::Fill(x, y, w, h) | Part::Frame(x, y, w, h) => {
                        assert!(x >= 0. && y >= 0. && x + w <= s && y + h <= s && w > 0. && h > 0., "{a:?}: {part:?}");
                    }
                    Part::Line(pts) | Part::Poly(pts) => {
                        assert!(pts.len() >= 2, "{a:?}");
                        for &(x, y) in pts {
                            assert!(x - half >= 0. && y - half >= 0. && x + half <= s && y + half <= s, "{a:?}: ({x}, {y})");
                        }
                    }
                }
            }
            assert!(!a.parts().is_empty(), "{a:?}");
        }
        assert_eq!(GO, Glyph::Fold);
    }

    /// A glyph is quads or strokes, never both; every stroked glyph (and
    /// only those) is a sprite the app's assets serve, its SVG written from
    /// its drawing.
    #[test]
    fn every_stroked_glyph_is_a_sprite() {
        let mut paths = Vec::new();
        for g in Glyph::ALL {
            let lines = g.parts().iter().filter(|p| matches!(p, Part::Line(_) | Part::Poly(_))).count();
            assert!(lines == 0 || lines == g.parts().len(), "{g:?} mixes quads and strokes");
            assert_eq!(g.sprite().is_some(), lines > 0, "{g:?}");
            let Some(path) = g.sprite() else { continue };
            assert!(!paths.contains(&path), "{path}");
            paths.push(path);
            let svg = Sprites.load(path).unwrap().unwrap_or_else(|| panic!("{path} not served"));
            assert_eq!(&*svg, svg_of(g).as_bytes());
            assert_eq!(svg_of(g).matches('M').count(), lines, "{g:?}: one subpath per line");
        }
        assert_eq!(paths.len(), 8);
        assert_eq!(
            svg_of(Glyph::Ok),
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10" viewBox="0 0 10 10"><path d="M1.5 5.5L4 8L8.5 2" fill="none" stroke="#000" stroke-width="1.5"/></svg>"##
        );
        assert_eq!(
            svg_of(Glyph::Close),
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8" viewBox="0 0 8 8"><path d="M1.6 1.6L6.4 6.4M6.4 1.6L1.6 6.4" fill="none" stroke="#000" stroke-width="1"/></svg>"##
        );
        assert!(Sprites.load("glyph/nope.svg").unwrap().is_none());
        assert!(Sprites.load("").unwrap().is_none());
        assert_eq!(Sprites.list("glyph/").unwrap().len(), 8);
        // The stars: one closed outline, joined round; the favourite's filled.
        let star = svg_of(Glyph::Star);
        assert!(star.contains(r#"L3.82 3.63Z" fill="none""#) && star.contains(r#"stroke-linejoin="round""#), "{star}");
        assert!(svg_of(Glyph::Starred).contains(r##"Z" fill="#000""##), "{}", svg_of(Glyph::Starred));
        assert!(!svg_of(Glyph::Ok).contains("linejoin"));
    }

    /// A write-on's cuts: served for every stroked glyph, drawn to their
    /// share of its strokes' length (`Failed`: the first stroke, then the
    /// second), the last cut the whole sprite; nothing else parses.
    #[test]
    fn write_on_cuts_are_sprites_of_their_share() {
        use super::{CUTS, svg_cut};
        for g in Glyph::ALL.into_iter().filter(|g| g.sprite().is_some()) {
            let base = g.sprite().unwrap().trim_end_matches(".svg");
            for k in 1..CUTS {
                let path = format!("{base}~{k}.svg");
                let svg = Sprites.load(&path).unwrap().unwrap_or_else(|| panic!("{path}"));
                assert_eq!(&*svg, svg_cut(g, k).as_bytes());
            }
            assert_eq!(svg_cut(g, CUTS), svg_of(g));
            assert!(Sprites.load(&format!("{base}~0.svg")).unwrap().is_none());
            assert!(Sprites.load(&format!("{base}~{CUTS}.svg")).unwrap().is_none());
        }
        // Half of `Failed` is its first stroke, whole; a quarter, half of it.
        assert!(svg_cut(Glyph::Failed, CUTS / 2).contains(r#"d="M2 2L8 8""#), "{}", svg_cut(Glyph::Failed, 4));
        assert!(svg_cut(Glyph::Failed, CUTS / 4).contains(r#"d="M2 2L5 5""#), "{}", svg_cut(Glyph::Failed, 2));
    }

    /// A call's state: the glyph never drops, ok has no word, and the
    /// families are the TUI's (cancelled is warn, as its subagent card).
    #[test]
    fn calls_map_to_one_glyph_word_and_family() {
        use ToolStatus as S;
        let c = |st, waiting, seen| call(st, waiting, seen);
        // Cards D (cards4/proposal.md §2, §4, an intended change): the
        // words in sentence case, running silent (its orbit and clock say it).
        assert_eq!(c(Some(S::Done), false, false), (Glyph::Ok, None, Family::Ok));
        assert_eq!(c(Some(S::Failed), false, false), (Glyph::Failed, Some("Failed"), Family::Err));
        assert_eq!(c(Some(S::Denied), true, false), (Glyph::Failed, Some("Denied"), Family::Err), "ended: a prompt no longer waits on it");
        assert_eq!(c(Some(S::Cancelled), false, false), (Glyph::Cancelled, Some("Cancelled"), Family::Warn));
        assert_eq!(c(Some(S::Unknown), false, false), (Glyph::Limit, Some("stopped at output limit"), Family::Warn));
        assert_eq!(c(Some(S::Queued), false, false), (Glyph::Idle, Some("Queued"), Family::Rest));
        assert_eq!(c(Some(S::Running), true, false), (Glyph::Needs, Some("Waiting for you"), Family::Warn));
        assert_eq!(c(None, true, true), (Glyph::Needs, Some("Waiting for you"), Family::Warn));
        assert_eq!(c(Some(S::Running), false, true), (Glyph::Idle, Some("Last seen running"), Family::Rest));
        assert_eq!(c(Some(S::Streaming), false, false), (Glyph::Run, None, Family::Run));
        // One life glyph each (§4), a worker's too.
        use super::{Life, life_of_call, life_of_worker};
        let lives: Vec<Life> = [Some(S::Done), Some(S::Failed), Some(S::Denied), Some(S::Cancelled), Some(S::Unknown), Some(S::Queued)]
            .into_iter()
            .map(|s| life_of_call(s, false, false))
            .chain([
                life_of_call(Some(S::Running), true, false),
                life_of_call(Some(S::Running), false, true),
                life_of_call(Some(S::Running), false, false),
            ])
            .collect();
        assert_eq!(
            lives,
            [Life::Done, Life::Failed, Life::Denied, Life::Cancelled, Life::Limit, Life::Queued, Life::Wait, Life::Seen, Life::Orbit]
        );
        assert_eq!(life_of_worker(Liveness::Running), Life::Orbit);
        assert_eq!(life_of_worker(Liveness::Done), Life::Done);
        assert_eq!(life_of_worker(Liveness::Failed), Life::Failed);
    }

    /// A worker's states: one glyph each (started and last seen share the
    /// rest glyph, as the table says), one word in each register.
    #[test]
    fn workers_map_to_one_glyph_and_word() {
        use crate::subagents::Status as S;
        let all = [
            Liveness::Started,
            Liveness::Running,
            Liveness::Cancelling,
            Liveness::Done,
            Liveness::Failed,
            Liveness::Cancelled,
            Liveness::TimedOut,
            Liveness::LastSeen,
            Liveness::Gone,
        ];
        let glyphs: Vec<Glyph> = all.iter().map(|&l| worker(l).0).collect();
        for (i, g) in glyphs.iter().enumerate() {
            let same: Vec<Liveness> = all.iter().zip(&glyphs).filter(|(_, h)| *h == g).map(|(l, _)| *l).collect();
            assert!(same.len() == 1 || same == [Liveness::Started, Liveness::LastSeen], "{:?} shares {g:?} with {same:?}", all[i]);
        }
        for l in all {
            let (_, display, mono, _) = worker(l);
            assert_eq!(
                display.replace("   ", "\u{0}").replace(' ', "").replace('\u{0}', " ").to_lowercase(),
                mono.replace(" running", ""),
                "{l:?}"
            );
        }
        assert_eq!(worker(Liveness::Done), (Glyph::Ok, "C O M P L E T E D", "completed", Family::Ok));
        assert_eq!(worker(Liveness::Cancelled).3, Family::Warn);
        assert_eq!(Liveness::of(Some(S::Running), false, true), Liveness::LastSeen);
        assert_eq!(Liveness::of(Some(S::Cancelling), true, true), Liveness::Cancelling);
        assert_eq!(Liveness::of(Some(S::Done), false, true), Liveness::Done, "how it ended stays so");
        assert_eq!(Liveness::of(None, true, true), Liveness::Started);
        assert_eq!(Liveness::of(Some(S::Started), true, true), Liveness::Started, "a start only in the history is not running");
        assert_eq!(Liveness::of(Some(S::Running), true, false), Liveness::Gone, "its session knows it no more");
        assert_eq!(Liveness::of(Some(S::Running), true, true).cancelling(true), Liveness::Cancelling);
        assert_eq!(Liveness::of(Some(S::Done), true, true).cancelling(true), Liveness::Done, "ended: a cancel too late changes nothing");
        assert_eq!(Liveness::of(Some(S::Started), true, true).cancelling(true), Liveness::Cancelling, "a re-attach's start, cancelled");
        assert!(Liveness::Gone.ended() && !Liveness::LastSeen.ended() && !Liveness::Started.ended());
    }

    fn bar(n: &[usize], h: u32) -> Vec<(usize, u32, u32)> {
        tally_bar(n, |&n| n, h).collect()
    }

    fn heights(n: &[usize], h: u32) -> Vec<u32> {
        bar(n, h).iter().map(|s| s.2).collect()
    }

    /// The stacked bar: 2 px each and a share of the rest ∝ the tally, the
    /// pixels flooring leaves to the largest fractional parts; segments
    /// touch and fill the bar exactly.
    #[test]
    fn the_tally_bar_stacks_its_counts() {
        // 7 tools, 36 px: 30·3/7 = 12 r 6/7 twice, 30/7 = 4 r 2/7; the 2 px
        // left go to the two 6/7s: 2 + 13, 2 + 13, 2 + 4.
        assert_eq!(bar(&[3, 3, 1], 36), [(0, 0, 15), (1, 15, 15), (2, 30, 6)]);
        assert_eq!(bar(&[4], 36), [(0, 0, 36)]);
        // 32·39/40 = 31.2, 32/40 = 0.8: the pixel left to the 0.8.
        assert_eq!(bar(&[39, 1], 36), [(0, 0, 33), (1, 33, 3)]);
        // The open summary's bar ends over the 1 px cut: 29·3/7 = 12 r 3/7
        // twice, 29/7 = 4 r 1/7, one pixel left: the first 3/7 (tally order).
        assert_eq!(heights(&[3, 3, 1], 35), [15, 14, 6]);
        // The finish gate's cases: a dominant tool and many singletons, and
        // 13 names (MCP tools): 9 drawn, the dominant one the tallest.
        let mut one_and_17 = vec![10];
        one_and_17.extend([1; 17]);
        assert_eq!(heights(&one_and_17, 36), [12, 3, 3, 3, 3, 3, 3, 3, 3]);
        // 18·nᵢ/56 = 6.43 3.86 2.57 1.61 1.29 0.96 0.64 0.32 0.32: 5 px left,
        // to .96 .86 .64 .61 .57.
        assert_eq!(heights(&[20, 12, 8, 5, 4, 3, 2, 1, 1, 1, 1, 1, 1], 36), [8, 6, 5, 4, 3, 3, 3, 2, 2]);
        // Shady's: equal counts drawn alike, never under a tool used less.
        let mut two_and_13 = vec![4, 4];
        two_and_13.extend([1; 13]);
        assert_eq!(heights(&two_and_13, 36), [7, 7, 4, 3, 3, 3, 3, 3, 3]);
        // The top tool has 2 of 10 calls: 5 px of 36, not a third of the bar.
        assert_eq!(heights(&[2, 1, 1, 1, 1, 1, 1, 1, 1], 36), [5, 4, 4, 4, 4, 4, 4, 4, 3]);
        // More tools than slots: the first h/4 drawn, the rest take no one's colour.
        let many = bar(&[1; 30], 36);
        assert_eq!(many.len(), 9);
        assert!(many.iter().all(|s| s.2 == 4));
        assert!(bar(&[], 36).is_empty());
    }

    /// Over random tallies (sorted, as a summary's are) at every summary
    /// height the steps give: tally order, heights never growing down it,
    /// at least 2 px each, at most h/4 drawn, touching, filling the bar;
    /// each within 1 px of its exact share, equal counts within 1 px.
    #[test]
    fn the_tally_bar_keeps_the_tally_order() {
        let mut x = 0x9e37_79b9_7f4a_7c15u64;
        let mut rnd = move |below: u64| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x % below
        };
        for _ in 0..20_000 {
            let tools = 1 + rnd(24) as usize;
            let mut n: Vec<usize> = (0..tools).map(|_| 1 + (rnd(40) * rnd(40) / 40) as usize).collect();
            n.sort_by_key(|&c| std::cmp::Reverse(c));
            for h in [27, 35, 36, 49] {
                let segs = bar(&n, h);
                assert_eq!(segs.len(), n.len().min(h as usize / 4), "{n:?} in {h}");
                assert_eq!(segs.iter().map(|s| s.2).sum::<u32>(), h, "{n:?} in {h}");
                let mut top = 0;
                for (k, &(i, t, s)) in segs.iter().enumerate() {
                    assert_eq!((i, t), (k, top), "{n:?} in {h}: in order, touching");
                    assert!(s >= 2, "{n:?} in {h}: {segs:?}");
                    top += s;
                }
                for w in segs.windows(2) {
                    assert!(w[0].2 >= w[1].2, "{n:?} in {h}: {segs:?} inverts");
                }
                let k = segs.len();
                let (free, sum) = ((h as usize - 2 * k) as f64, n[..k].iter().sum::<usize>() as f64);
                for (&(i, _, s), &c) in segs.iter().zip(&n) {
                    let exact = 2. + free * c as f64 / sum;
                    assert!((s as f64 - exact).abs() < 1., "{n:?} in {h}: segment {i} is {s}, its share {exact:.2}");
                }
                for (a, sa) in segs.iter().enumerate() {
                    for sb in segs[a + 1..].iter().filter(|sb| n[sb.0] == n[a]) {
                        assert!(sa.2.abs_diff(sb.2) <= 1, "{n:?} in {h}: equal counts {segs:?}");
                    }
                }
            }
        }
    }
}
