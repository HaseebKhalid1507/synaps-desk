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
}

impl Glyph {
    pub(crate) const ALL: [Glyph; 17] = [
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
    ];

    /// Its box: 10 px; `Close` 8.
    pub(crate) fn size(self) -> f32 {
        if self == Glyph::Close { 8. } else { 10. }
    }

    fn stroke(self) -> f32 {
        if self == Glyph::Close { 1. } else { 1.5 }
    }

    /// A stroked glyph's sprite: its SVG's asset path ([`Sprites`]).
    pub(crate) fn sprite(self) -> Option<&'static str> {
        Some(match self {
            Glyph::Ok => "glyph/ok.svg",
            Glyph::Failed => "glyph/failed.svg",
            Glyph::Fold => "glyph/fold.svg",
            Glyph::FoldOpen => "glyph/fold-open.svg",
            Glyph::Close => "glyph/close.svg",
            _ => return None,
        })
    }

    pub(crate) fn parts(self) -> &'static [Part] {
        use Part::{Fill, Frame, Line};
        match self {
            Glyph::Run => &[Fill(1., 1., 8., 8.)],
            Glyph::Workers => &[Fill(1., 1., 4., 4.), Fill(5., 5., 4., 4.)],
            Glyph::Needs => &[Frame(0., 0., 10., 10.), Fill(3., 3., 4., 4.)],
            Glyph::Unread => &[Fill(3., 3., 4., 4.)],
            Glyph::Idle => &[Frame(1., 1., 8., 8.)],
            Glyph::Parked => &[Fill(1., 7., 8., 2.)],
            Glyph::Opening => &[Fill(1., 1., 8., 1.), Fill(1., 1., 1., 8.)],
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
        }
    }
}

/// `g` in `color`: its quads, or its sprite tinted.
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
            Part::Line(_) => el,
        };
    }
    el.into_any_element()
}

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
    let lines = || g.parts().iter().filter_map(|p| if let Part::Line(pts) = p { Some(*pts) } else { None });
    let total: f32 = lines().map(|pts| pts.windows(2).map(|w| seg(w[0], w[1])).sum::<f32>()).sum();
    let mut left = total * k.min(CUTS) as f32 / CUTS as f32;
    let mut d = String::new();
    for pts in lines() {
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
                let (x, y) = (w[0].0 + (w[1].0 - w[0].0) * t, w[0].1 + (w[1].1 - w[0].1) * t);
                let _ = write!(d, "L{} {}", (x * 100.).round() / 100., (y * 100.).round() / 100.);
                left = 0.;
                break;
            }
        }
    }
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{s}" height="{s}" viewBox="0 0 {s} {s}"><path d="{d}" fill="none" stroke="#000" stroke-width="{}"/></svg>"##,
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
            Part::Line(_) => el,
        };
    }
    el.into_any_element()
}

/// The scan's row in the glyph box at `beat` (motion.md §7.1): one device
/// pixel down per beat, a 1 s cycle.
pub(crate) fn scan_row(beat: u32) -> f32 {
    1. + (beat % 8) as f32
}

/// A running glyph (`Run`, `Workers`) at `beat`: its quads, and its beam
/// ([`beam_rows`]). Any other glyph: [`draw`].
pub(crate) fn scan(g: Glyph, beat: Option<u32>, mark: Hsla, ink: Hsla) -> AnyElement {
    let Some(beat) = beat.filter(|_| matches!(g, Glyph::Run | Glyph::Workers)) else { return draw(g, mark) };
    let mut el = div().relative().flex_none().size(px(g.size()));
    for &part in g.parts() {
        if let Part::Fill(x, top, w, h) = part {
            el = el.child(div().absolute().left(px(x)).top(px(top)).w(px(w)).h(px(h)).bg(mark));
        }
    }
    let hot = beam(mark, ink).0;
    for (x, y, w, c) in beam_rows(g, beat, mark, ink) {
        let row = div().absolute().left(px(x)).top(px(y)).w(px(w)).h(px(1.)).bg(c);
        el = el.child(if c == hot { row.debug_selector(|| "beam-row".into()) } else { row });
    }
    el.into_any_element()
}

/// The scan's phosphor beam in `g`'s box at `beat` (motion.md §10
/// amendment 14): the 1 px row the beat is on, lit toward the ink
/// (`mix(mark, ink, 0.6)`), and the row it left one beat ago, its
/// afterglow (`mix(mark, ink, 0.22)`; none as the beam wraps to the top).
/// x, y, width, colour: one per fill a row crosses. [`scan`] and the beat
/// layer draw these.
pub(crate) fn beam_rows(g: Glyph, beat: u32, mark: Hsla, ink: Hsla) -> impl Iterator<Item = (f32, f32, f32, Hsla)> {
    let (hot, glow) = beam(mark, ink);
    let trail = (!beat.is_multiple_of(8)).then(|| scan_row(beat - 1));
    rows_at(g, scan_row(beat), hot).chain(trail.into_iter().flat_map(move |y| rows_at(g, y, glow)))
}

fn rows_at(g: Glyph, y: f32, c: Hsla) -> impl Iterator<Item = (f32, f32, f32, Hsla)> {
    g.parts().iter().filter_map(move |&part| match part {
        Part::Fill(x, top, w, h) if (top..top + h).contains(&y) => Some((x, y, w, c)),
        _ => None,
    })
}

/// The beam's row and its afterglow's: the mark toward the ink.
pub(crate) fn beam(mark: Hsla, ink: Hsla) -> (Hsla, Hsla) {
    (crate::motion::mix(mark, ink, BEAM), crate::motion::mix(mark, ink, GLOW))
}

/// How far toward the ink the beam's row is lit, and its afterglow's.
pub(crate) const BEAM: f32 = 0.6;
pub(crate) const GLOW: f32 = 0.22;

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
/// drawn still (a scan's quads without its row; a breath's cell at
/// `mark`, where it ends), and where it was painted recorded for the
/// layer. Pixel for pixel what [`scan`] and [`breath`] draw, the layer's
/// row or cell painted over it.
pub(crate) fn on_beat(beat: crate::motion::Beat, mark: Hsla, ground: Hsla, fx: &std::rc::Rc<crate::motion::Motion>) -> AnyElement {
    let el = match beat {
        crate::motion::Beat::Scan(g) => {
            let mut el = div().relative().flex_none().size(px(g.size()));
            for &part in g.parts() {
                if let Part::Fill(x, top, w, h) = part {
                    el = el.child(div().absolute().left(px(x)).top(px(top)).w(px(w)).h(px(h)).bg(mark));
                }
            }
            el
        }
        crate::motion::Beat::Breath(_) => breath_box(mark, mark),
    };
    let (fx, ink) = (fx.clone(), fx.ink());
    el.child(
        canvas(
            |_, _, _| {},
            move |b, _, window, _| fx.site(crate::motion::Site { origin: b.origin, mask: window.content_mask(), mark, ground, ink, beat }),
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
        return draw(to, mark);
    }
    let draws = match to {
        Glyph::Ok | Glyph::Failed => true,
        Glyph::Cancelled | Glyph::Limit | Glyph::Cancelling => from.is_none_or(|(g, _)| g != Glyph::Cancelling),
        _ => false,
    };
    if draws && !fade {
        return draw_at(to, p, mark);
    }
    let s = px(to.size().max(from.map_or(0., |(g, _)| g.size())));
    div()
        .relative()
        .flex_none()
        .size(s)
        .children(from.filter(|(g, _)| *g != to).map(|(g, c)| div().absolute().inset_0().child(draw(g, crate::motion::mix(c, ground, p)))))
        .child(div().absolute().inset_0().child(draw(to, crate::motion::mix(ground, mark, p))))
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

/// A lit bar's bloom: a 1 px column of `color` on the bar's right (x 2),
/// drawn to `p` of its length as the bar is. Paint only.
pub(crate) fn bloom_at(color: Hsla, p: f32) -> Div {
    div().debug_selector(|| "bar-bloom".into()).absolute().left(px(2.)).top_0().w(px(1.)).h(relative(p.clamp(0., 1.))).bg(color)
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

/// A session's glyph on `ground`, in a 10 px box (empty when it has none).
pub(crate) fn session_mark(st: State, ground: Ground, p: &Palette) -> Div {
    let (g, f) = session(st);
    let (_, mark) = ink(f, ground, p);
    div().flex().flex_none().size(px(10.)).children(g.map(|g| draw(g, mark)))
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
        Some(ToolStatus::Failed) => (Glyph::Failed, Some("failed"), Family::Err),
        Some(ToolStatus::Denied) => (Glyph::Failed, Some("denied"), Family::Err),
        Some(ToolStatus::Cancelled) => (Glyph::Cancelled, Some("cancelled"), Family::Warn),
        // The daemon stopped it at its output limit: neither ok nor failed.
        Some(ToolStatus::Unknown) => (Glyph::Limit, Some("stopped at limit"), Family::Warn),
        Some(ToolStatus::Queued) => (Glyph::Idle, Some("queued"), Family::Rest),
        _ if waiting => (Glyph::Needs, Some("waiting"), Family::Warn),
        _ if seen => (Glyph::Idle, Some("last seen running"), Family::Rest),
        _ => (Glyph::Run, Some("running"), Family::Run),
    }
}

/// A folded run's trail glyph: its worst outcome, failed (or denied) >
/// stopped at the limit > cancelled > ok.
pub(crate) fn worst(failed: usize, stopped: usize, cancelled: usize) -> (Glyph, Family) {
    if failed > 0 {
        (Glyph::Failed, Family::Err)
    } else if stopped > 0 {
        (Glyph::Limit, Family::Warn)
    } else if cancelled > 0 {
        (Glyph::Cancelled, Family::Warn)
    } else {
        (Glyph::Ok, Family::Ok)
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

#[cfg(test)]
mod tests {
    use super::{Family, GO, Glyph, Part, Sprites, call, svg_of, tally_bar, worker, worst};
    use crate::subagents::Liveness;
    use gpui_kit::AssetSource;
    use synaps_client::transcript::ToolStatus;

    /// The scan's beam (motion.md §10 amendment 14): on every beat a
    /// running glyph's lit row and, one row above it, the row it left, its
    /// afterglow: two quads a beat for `Run` (one as the beam wraps back to
    /// the top), lit toward the ink.
    #[test]
    fn the_beam_lights_its_row_and_trails_one() {
        let mark = gpui_kit::Hsla { h: 0.6, s: 0.5, l: 0.4, a: 1. };
        let ink = gpui_kit::Hsla { h: 0., s: 0., l: 0.95, a: 1. };
        let (hot, glow) = super::beam(mark, ink);
        for beat in 0..24u32 {
            let rows: Vec<_> = super::beam_rows(Glyph::Run, beat, mark, ink).collect();
            let y = super::scan_row(beat);
            assert_eq!(rows[0], (1., y, 8., hot), "beat {beat}");
            if beat % 8 == 0 {
                assert_eq!(rows.len(), 1, "beat {beat}: wrapping, no trail across the glyph");
            } else {
                assert_eq!(rows[1..], [(1., y - 1., 8., glow)], "beat {beat}");
            }
        }
        assert!(hot.l > glow.l && glow.l > mark.l, "lit toward the ink");
        assert_eq!(super::beam_rows(Glyph::Workers, 3, mark, ink).count() % 2, 0, "every fill the beam crosses trails too");
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
                    Part::Line(pts) => {
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
            let lines = g.parts().iter().filter(|p| matches!(p, Part::Line(_))).count();
            assert!(lines == 0 || lines == g.parts().len(), "{g:?} mixes quads and strokes");
            assert_eq!(g.sprite().is_some(), lines > 0, "{g:?}");
            let Some(path) = g.sprite() else { continue };
            assert!(!paths.contains(&path), "{path}");
            paths.push(path);
            let svg = Sprites.load(path).unwrap().unwrap_or_else(|| panic!("{path} not served"));
            assert_eq!(&*svg, svg_of(g).as_bytes());
            assert_eq!(svg_of(g).matches('M').count(), lines, "{g:?}: one subpath per line");
        }
        assert_eq!(paths.len(), 5);
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
        assert_eq!(Sprites.list("glyph/").unwrap().len(), 5);
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
        assert_eq!(super::scan_row(0), 1.);
        assert_eq!(super::scan_row(7), 8.);
        assert_eq!(super::scan_row(8), 1.);
    }

    /// A call's state: the glyph never drops, ok has no word, and the
    /// families are the TUI's (cancelled is warn, as its subagent card).
    #[test]
    fn calls_map_to_one_glyph_word_and_family() {
        use ToolStatus as S;
        let c = |st, waiting, seen| call(st, waiting, seen);
        assert_eq!(c(Some(S::Done), false, false), (Glyph::Ok, None, Family::Ok));
        assert_eq!(c(Some(S::Failed), false, false), (Glyph::Failed, Some("failed"), Family::Err));
        assert_eq!(c(Some(S::Denied), true, false), (Glyph::Failed, Some("denied"), Family::Err), "ended: a prompt no longer waits on it");
        assert_eq!(c(Some(S::Cancelled), false, false), (Glyph::Cancelled, Some("cancelled"), Family::Warn));
        assert_eq!(c(Some(S::Unknown), false, false), (Glyph::Limit, Some("stopped at limit"), Family::Warn));
        assert_eq!(c(Some(S::Queued), false, false), (Glyph::Idle, Some("queued"), Family::Rest));
        assert_eq!(c(Some(S::Running), true, false), (Glyph::Needs, Some("waiting"), Family::Warn));
        assert_eq!(c(None, true, true), (Glyph::Needs, Some("waiting"), Family::Warn));
        assert_eq!(c(Some(S::Running), false, true), (Glyph::Idle, Some("last seen running"), Family::Rest));
        assert_eq!(c(Some(S::Streaming), false, false), (Glyph::Run, Some("running"), Family::Run));
        assert_eq!(worst(1, 1, 1), (Glyph::Failed, Family::Err));
        assert_eq!(worst(0, 1, 1), (Glyph::Limit, Family::Warn));
        assert_eq!(worst(0, 0, 1), (Glyph::Cancelled, Family::Warn));
        assert_eq!(worst(0, 0, 0), (Glyph::Ok, Family::Ok));
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
