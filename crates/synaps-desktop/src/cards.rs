//! Cards D (cards4/proposal.md §1–§4): a run of tool calls is one
//! square card (Haseeb, #23 r7; square everywhere since: no box or
//! button in the app has a radius, only true circles, a status dot or
//! disc, are round) on `card_d`, read by its fill alone (no
//! outline: Haseeb, 07:18 "remove border outlines from tool boxes"); each call is
//! one quiet 34 px row inside it (a life glyph, the verb in Sans, the
//! target in mono, the facts right in small grey Sans, a state word, the
//! chevron); its body sits in a `well_d` lined up under the verb. The list
//! is virtualized, so a run's card is drawn by its rows: the first carries
//! its top edge and corners, the last its bottom ([`edges`]).

use std::time::Duration;

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::fonts::TextPx as _;

use crate::theme::{Metrics, Palette};
use crate::turns::{Disp, Gap, Kind};

/// A row's box and its parts (§3), at the text step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Geo {
    /// The card's inner padding (its rows sit this far in).
    pub inset: f32,
    /// Where the card's 1 px hairline was, padding now: the card has no
    /// outline, and nothing in it moved when it went.
    pub edge: f32,
    pub row: f32,
    pub pad: f32,
    pub gap: f32,
    pub glyph: f32,
    pub verb: f32,
    /// A body's indent from the row's edge: the verb column.
    pub indent: f32,
    pub chevron: f32,
}

impl Geo {
    pub(crate) fn of(m: &Metrics) -> Self {
        let px = |v: f32| (v * m.r).round();
        let (pad, gap, glyph) = (px(10.), px(10.), crate::glyph::LIFE);
        Geo { inset: 3., edge: 1., row: px(34.), pad, gap, glyph, verb: px(56.), indent: pad + glyph + gap, chevron: 10. }
    }
}

/// A row inside a card: `card_d` until hovered (`hover`, colour only,
/// nothing shifts), on its wash when it has one.
pub(crate) fn in_card(d: &Disp) -> bool {
    matches!(d.kind, Kind::Work | Kind::Summary { .. })
}

/// Which of its card's edges row `ix` carries: the top (a run's first row,
/// or the first after its prompt's card) and the bottom (its last).
pub(crate) fn edges(disp: &[Disp], ix: usize) -> (bool, bool) {
    let Some(d) = disp.get(ix).filter(|d| in_card(d)) else { return (false, false) };
    let top = d.gap != Gap::None || ix == 0 || !in_card(&disp[ix - 1]);
    let bottom = disp.get(ix + 1).is_none_or(|n| !in_card(n) || n.gap != Gap::None);
    (top, bottom)
}

/// A row's `rev` with its edges in it: a row that gains or loses an edge
/// (the run grew a call; a prompt's card came under it) is measured again.
pub(crate) const EDGE_TOP: u64 = 1 << 61;
pub(crate) const EDGE_BOTTOM: u64 = 1 << 60;

/// Every card row of `out` marked with its edges ([`EDGE_TOP`], [`EDGE_BOTTOM`]).
pub(crate) fn mark_edges(mut out: Vec<Disp>) -> Vec<Disp> {
    let marks: Vec<(bool, bool)> = (0..out.len()).map(|ix| edges(&out, ix)).collect();
    for (d, (top, bottom)) in out.iter_mut().zip(marks) {
        if top {
            d.rev ^= EDGE_TOP;
        }
        if bottom {
            d.rev ^= EDGE_BOTTOM;
        }
    }
    out
}

/// A card's slice for one list row (§3): `card_d`, square, no outline; its
/// top edge on its first row, its bottom on its last. Down both
/// sides and on those edges, two paddings: the old hairline's 1 px and the
/// 3 px inset, in two layers so each is snapped to device pixels on its own
/// as the border and the inset were (Shady #71 P2: one 4 px padding is 6
/// device px at 150 %, the two were 1 + 4). The hairline's room is at
/// least a device pixel, as a border is snapped (Shady #71 P3: at 50 % and
/// under, 1 px of padding is half a pixel, rounded away): `scale`, the
/// window's. Its rows sit where they sat in the outlined card at every
/// scale.
pub(crate) fn slab(top: bool, bottom: bool, body: impl IntoElement, p: &Palette, g: Geo, scale: f32) -> Div {
    let edge = if g.edge > 0. { g.edge.max(1. / scale) } else { 0. };
    let pad = |d: Div, w: f32| d.px(px(w)).when(top, |d| d.pt(px(w))).when(bottom, |d| d.pb(px(w)));
    pad(div().w_full().bg(p.card_d), edge).child(pad(div(), g.inset).child(body))
}

/// Where a 1 px frame was, padding now (no outlines, Haseeb): its room at
/// `scale`, a device pixel at least, as a border is snapped (Shady #71
/// P3). Padded in a layer of its own, apart from the padding inside it, as
/// the frame was laid out (P2): one padding of 1 + 10 is 19 device px at
/// 175 %, a frame and its 10 were 2 + 18.
pub(crate) fn frame_room(scale: f32) -> f32 {
    1f32.max(1. / scale)
}

/// An opened summary: its row, the `line` rule under it where its card's
/// bottom edge was, and the inset under that, so it stands as tall open as
/// folded (its card's bottom edge goes to the run's last row): nothing
/// below it moves.
pub(crate) fn opened(row: impl IntoElement, id: u64, p: &Palette, g: Geo) -> Div {
    v_flex().child(row).child(div().h(px(g.edge)).bg(p.line).debug_selector(move || format!("run-rule {id}"))).pb(px(g.inset))
}

/// A fact's separator (§3): a 3 px dot (a true circle) in `dim` at 55 %,
/// centred.
pub(crate) fn dot(p: &Palette) -> Div {
    div().flex_none().size(px(3.)).rounded(px(1.5)).bg(crate::motion::mix(p.card_d, p.dim_t, 0.55))
}

/// Facts in order, a dot between each two.
pub(crate) fn facts(parts: Vec<AnyElement>, p: &Palette, m: &Metrics) -> Div {
    let mut el = h_flex().flex_none().gap(px(6.)).items_center().font_family(crate::fonts::SANS).text_px(m.meta).whitespace_nowrap();
    for (i, f) in parts.into_iter().enumerate() {
        if i > 0 {
            el = el.child(dot(p));
        }
        el = el.child(f);
    }
    el
}

/// A tool's verb in a card (§2, C6 bent): a plain Sans word, sentence case.
pub(crate) fn verb(name: &str) -> SharedString {
    let word = match name {
        "bash" => "Run",
        "read" => "Read",
        "write" => "Write",
        "edit" | "multi_edit" => "Edit",
        "grep" | "web_search" => "Search",
        "find" | "glob" => "Find",
        "ls" => "List",
        "web_fetch" | "fetch" => "Fetch",
        "subagent_start" | "subagent" => "Spawn",
        "subagent_collect" => "Collect",
        "todo_write" => "Todos",
        "activate_tools" => "Activate",
        // An agent card's call drawn as a tool row: one this desk can't
        // read (synaps-cards design §6.3), or a close.
        "cards_ui" | "cards:ui" => "Ask",
        n if n.starts_with("ext__") => "Tool",
        // Anything else: its first word (Shady #23 L6), so it fits the
        // column: `subagent_steer` → "Steer", `load_skill` → "Load".
        n => {
            let n = n.strip_prefix("subagent_").unwrap_or(n);
            let word = n.split(['_', '-', ' ']).find(|w| !w.is_empty()).unwrap_or(n);
            let mut c = word.chars();
            return c.next().map_or_else(SharedString::default, |f| f.to_uppercase().chain(c).collect::<String>().into());
        }
    };
    word.into()
}

/// How a failed shell command ended (§4), from the engine's own header,
/// `Command failed (exit N):` on its result's first line, and for `bash`
/// only (Shady #23 M7: no other tool's text, nor the output under it, is
/// read for an exit). The engine says `exit -1` for a command a signal
/// killed: "Killed".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Exit {
    Code(i64),
    Killed,
}

impl Exit {
    pub(crate) fn of(tool: &str, result: &str) -> Option<Exit> {
        if tool != "bash" {
            return None;
        }
        const HEAD: &str = "command failed (exit ";
        let first = result.lines().next()?;
        let at = first.to_ascii_lowercase().find(HEAD)? + HEAD.len();
        let rest = &first[at..];
        let end = rest.find(')')?;
        match rest[..end].trim().parse::<i64>().ok()? {
            n if n < 0 => Some(Exit::Killed),
            n => Some(Exit::Code(n)),
        }
    }

    pub(crate) fn word(self) -> SharedString {
        match self {
            Exit::Code(n) => format!("Exit {n}").into(),
            Exit::Killed => "Killed".into(),
        }
    }
}

/// A call's duration as its facts say it (§3): `0.4s`, `3.2s`, `12s`, `1m 23s`.
pub(crate) fn took(d: Duration) -> SharedString {
    let s = d.as_secs_f64();
    if s < 10. {
        format!("{s:.1}s").into()
    } else if s < 60. {
        format!("{}s", s as u64).into()
    } else {
        format!("{}m {}s", s as u64 / 60, s as u64 % 60).into()
    }
}

/// The button style of Allow (`authorize.rs`): the accent filled (Haseeb's
/// rule, "attention follows the accent"), or outlined in it. One switch:
/// [`crate::authorize::ALLOW_STYLE`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AllowStyle {
    Fill,
    // The other side of the switch: painted and tested.
    #[cfg_attr(not(test), allow(dead_code))]
    Outline,
}

impl AllowStyle {
    /// Its fill, edge and label at rest, hovered and pressed: `(bg, edge, text)`.
    pub(crate) fn colors(self, p: &Palette, hovered: bool, pressed: bool) -> (Hsla, Hsla, Hsla) {
        match self {
            AllowStyle::Fill => {
                let bg = if pressed {
                    crate::motion::mix(p.btn_fill, p.ink, 0.18)
                } else if hovered {
                    p.btn_hi
                } else {
                    p.btn_fill
                };
                (bg, bg, p.acc_ink)
            }
            AllowStyle::Outline => {
                let bg = if pressed {
                    p.out_press
                } else if hovered {
                    p.acc_soft
                } else {
                    p.card_d
                };
                (bg, p.acc_line, p.acc)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AllowStyle, EDGE_BOTTOM, EDGE_TOP, Exit, edges, mark_edges, took, verb};
    use crate::turns::{Disp, Gap, Kind, Who};
    use std::time::Duration;

    fn d(kind: Kind, gap: Gap) -> Disp {
        Disp { item: 0, id: 1, rev: 0, kind, gap, who: Who::None }
    }

    #[test]
    fn a_runs_card_is_drawn_by_its_first_and_last_rows() {
        let rows = [
            d(Kind::Prose, Gap::First),
            d(Kind::Work, Gap::Part),
            d(Kind::Work, Gap::None),
            d(Kind::Work, Gap::None),
            d(Kind::Prompt, Gap::None),
            d(Kind::Work, Gap::None),
            d(Kind::Prose, Gap::Part),
            d(Kind::Summary { end: 3, open: false }, Gap::Part),
        ];
        let e: Vec<(bool, bool)> = (0..rows.len()).map(|i| edges(&rows, i)).collect();
        assert_eq!(
            e,
            [(false, false), (true, false), (false, false), (false, true), (false, false), (true, true), (false, false), (true, true)]
        );
        let marked = mark_edges(rows.to_vec());
        assert_eq!(marked[1].rev, EDGE_TOP);
        assert_eq!(marked[3].rev, EDGE_BOTTOM);
        assert_eq!(marked[5].rev, EDGE_TOP | EDGE_BOTTOM);
        assert_eq!(marked[2].rev, 0);
    }

    /// No outline (Haseeb, 07:18): a card's slice draws no border, and
    /// its padding is the old hairline's 1 px around the 3 px inset, two
    /// layers where the border and the inset were, so no row moved when it
    /// went. The hairline's room is a device pixel at least (Shady #71 P3):
    /// 2 px at 50 %, 4 at 25 %.
    #[test]
    fn a_card_reads_by_its_fill_alone() {
        use gpui_kit::{DefiniteLength, Div, Styled as _, div, px};
        let g = super::Geo::of(&crate::theme::Metrics::at(crate::theme::DEFAULT_TEXT));
        assert_eq!((g.edge, g.inset), (1., 3.));
        let p = crate::theme::Palette::of(&crate::theme::resolve(&crate::palettes::THEMES[0].1, None).0);
        let pads = |d: &mut Div, w: f32, top: bool, bottom: bool| {
            let s = d.style();
            let b = &s.border_widths;
            assert!([b.top, b.right, b.bottom, b.left].iter().all(Option::is_none), "({top}, {bottom}) {w}: no outline");
            assert_eq!(s.border_color, None);
            let pad = Some(DefiniteLength::from(px(w)));
            let e = &s.padding;
            assert_eq!((e.left, e.right), (pad, pad), "({top}, {bottom}) {w}: both sides");
            assert_eq!((e.top, e.bottom), (top.then_some(pad).flatten(), bottom.then_some(pad).flatten()), "({top}, {bottom}) {w}");
        };
        for (top, bottom) in [(false, false), (true, false), (false, true), (true, true)] {
            for (scale, edge) in [(1., 1.), (1.5, 1.), (2., 1.), (0.75, 1. / 0.75), (0.5, 2.), (0.25, 4.)] {
                pads(&mut super::slab(top, bottom, div(), &p, g, scale), edge, top, bottom);
            }
        }
    }

    /// Shady #71 P2: GPUI snaps a border and a padding to device pixels
    /// each on its own (a border to at least one), so one 4 px padding is
    /// not the hairline's 1 and the inset's 3 at 150 % and 250 % (6 device
    /// px for 1 + 4, 10 for 2 + 7). Laid out at 25 to 250 %, every slice
    /// (each pair of edges) holds its row where main's outlined slice held
    /// it, as wide and as tall: at 50 % and under too, where a border is
    /// held to a device pixel and a padding is not (P3). A run's opened
    /// summary (its rule) is laid out as main's, and from 100 % as tall as
    /// its folded one (its card's bottom edge); under that it was not on
    /// main either (the rule is a 1 px height, no stroke).
    #[gpui_kit::test]
    fn a_card_keeps_its_outlined_geometry_at_every_scale(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::prelude::FluentBuilder as _;
        use gpui_kit::{
            Context, Div, InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Render, Styled as _, Window, div, px,
        };
        const CAPS: [(bool, bool); 4] = [(false, false), (true, false), (false, true), (true, true)];
        struct Cards(crate::theme::Palette, super::Geo);
        impl Render for Cards {
            fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let (p, g, scale) = (&self.0, self.1, window.scale_factor());
                let row = |s: String| div().w_full().h(px(g.row)).debug_selector(move || s);
                // Main's slice (9659a1a): the `line` hairline and the inset.
                let outlined = |top: bool, bottom: bool, body: Div| {
                    div()
                        .w_full()
                        .bg(p.card_d)
                        .border_color(p.line)
                        .border_l_1()
                        .border_r_1()
                        .px(px(g.inset))
                        .when(top, |d| d.border_t_1().pt(px(g.inset)))
                        .when(bottom, |d| d.border_b_1().pb(px(g.inset)))
                        .child(body)
                };
                let mut col = gpui_kit::component::v_flex().w(px(701.));
                for (k, (top, bottom)) in CAPS.into_iter().enumerate() {
                    col = col
                        .child(outlined(top, bottom, row(format!("old row {k}"))).debug_selector(move || format!("old card {k}")))
                        .child(
                            super::slab(top, bottom, row(format!("new row {k}")), p, g, scale)
                                .debug_selector(move || format!("new card {k}")),
                        );
                }
                // Main's opened summary: its row, the 1 px rule, the inset.
                let old_opened =
                    gpui_kit::component::v_flex().child(row("old opened row".into())).child(div().h(px(1.)).bg(p.line)).pb(px(3.));
                col.child(super::slab(true, true, row("folded row".into()), p, g, scale).debug_selector(|| "folded card".into()))
                    .child(
                        super::slab(true, false, super::opened(row("opened row".into()), 7, p, g), p, g, scale)
                            .debug_selector(|| "opened card".into()),
                    )
                    .child(outlined(true, false, old_opened).debug_selector(|| "old opened card".into()))
            }
        }
        let g = super::Geo::of(&crate::theme::Metrics::at(crate::theme::DEFAULT_TEXT));
        let p = crate::theme::Palette::of(&crate::theme::resolve(&crate::palettes::THEMES[0].1, None).0);
        let (view, cx) = cx.add_window_view(|_, _| Cards(p, g));
        for scale in [1., 1.25, 1.5, 1.75, 2., 2.5, 0.5, 0.25] {
            cx.simulate_scale_factor_change(scale);
            view.update(cx, |_, cx| cx.notify());
            cx.run_until_parked();
            assert_eq!(cx.update(|window, _| window.scale_factor()), scale);
            let dev = |v: Pixels| (f32::from(v) * scale).round() as i32;
            // A card's size, and its row's offset in it and size, in device px.
            let mut boxes = |card: String, row: String| {
                let c = cx.debug_bounds(card.clone().leak()).unwrap_or_else(|| panic!("{scale}: {card} drawn"));
                let r = cx.debug_bounds(row.clone().leak()).unwrap_or_else(|| panic!("{scale}: {row} drawn"));
                let (cx0, cy0) = (dev(c.origin.x), dev(c.origin.y));
                [dev(c.size.width), dev(c.size.height), dev(r.origin.x) - cx0, dev(r.origin.y) - cy0, dev(r.size.width), dev(r.size.height)]
            };
            for (k, caps) in CAPS.into_iter().enumerate() {
                let old = boxes(format!("old card {k}"), format!("old row {k}"));
                assert_eq!(boxes(format!("new card {k}"), format!("new row {k}")), old, "{scale} {caps:?}: as the outlined slice");
            }
            let folded = boxes("folded card".into(), "folded row".into());
            let opened = boxes("opened card".into(), "opened row".into());
            assert_eq!(folded, boxes("old card 3".into(), "old row 3".into()), "{scale}: folded, as the outlined card");
            assert_eq!(opened, boxes("old opened card".into(), "old opened row".into()), "{scale}: opened, as main's");
            if scale >= 1. {
                assert_eq!(opened, folded, "{scale}: opened, as tall as folded");
            }
        }
    }

    /// A worker chip's frame went (no outlines) and its room is padding in
    /// a layer of its own ([`super::frame_room`]): laid out at 25 to 250 %,
    /// what is in the chip sits where it sat in the framed chip, as wide and
    /// as tall (Shady #71's lesson: one 1 + 10 padding is 19 device px at
    /// 175 %, the frame and its 10 were 2 + 18).
    #[gpui_kit::test]
    fn a_chip_keeps_its_framed_geometry_at_every_scale(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::component::{h_flex, v_flex};
        use gpui_kit::{Context, InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Render, Styled as _, Window, div, px};
        struct Chips;
        impl Render for Chips {
            fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let body = |s: &'static str| div().flex_1().min_w_0().h(px(10.)).debug_selector(move || s.into());
                // Main's chip (7135205): a 1 px frame, then 12 and 10.
                let old = h_flex().w(px(301.)).h(px(27.)).border_1().pl(px(12.)).pr(px(10.)).child(body("old body"));
                let new = h_flex()
                    .w(px(301.))
                    .h(px(27.))
                    .px(px(super::frame_room(window.scale_factor())))
                    .child(h_flex().flex_1().min_w_0().h_full().pl(px(12.)).pr(px(10.)).child(body("new body")));
                v_flex().child(old.debug_selector(|| "old chip".into())).child(new.debug_selector(|| "new chip".into()))
            }
        }
        let (view, cx) = cx.add_window_view(|_, _| Chips);
        for scale in [1., 1.25, 1.5, 1.75, 2., 2.5, 0.5, 0.25] {
            cx.simulate_scale_factor_change(scale);
            view.update(cx, |_, cx| cx.notify());
            cx.run_until_parked();
            let dev = |v: Pixels| (f32::from(v) * scale).round() as i32;
            // The chip's size, and its body's offset in it and size, in device px.
            let mut boxes = |chip: &'static str, body: &'static str| {
                let c = cx.debug_bounds(chip).unwrap_or_else(|| panic!("{scale}: {chip} drawn"));
                let b = cx.debug_bounds(body).unwrap_or_else(|| panic!("{scale}: {body} drawn"));
                let (x0, y0) = (dev(c.origin.x), dev(c.origin.y));
                [dev(c.size.width), dev(c.size.height), dev(b.origin.x) - x0, dev(b.origin.y) - y0, dev(b.size.width), dev(b.size.height)]
            };
            let old = boxes("old chip", "old body");
            assert_eq!(boxes("new chip", "new body"), old, "{scale}: as the framed chip");
        }
    }

    /// The message box's frame went too (Haseeb: no outline on the
    /// composer, as on the cards): its room is padding in a layer of its
    /// own ([`crate::app::composer_box`]). Laid out at 25 to 250 %, in every
    /// form, the box is as wide and as tall as main's framed one; its text
    /// and its foot, the composer's top as the palette reads it, the bar,
    /// the top edge's pass and Handoff's light (part way down) all lie
    /// where they lay on main, as wide and as tall.
    #[gpui_kit::test]
    fn the_composer_keeps_its_framed_geometry_at_every_scale(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::component::v_flex;
        use gpui_kit::{
            Context, InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Render, Styled as _, Window, div, px, relative,
        };
        const FORMS: [crate::fit::Form; 3] = [crate::fit::Form::Full, crate::fit::Form::Short, crate::fit::Form::Cramped];
        struct Boxes(crate::theme::Metrics);
        impl Render for Boxes {
            fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let m = &self.0;
                let mut col = v_flex().w(px(701.));
                for (k, form) in FORMS.into_iter().enumerate() {
                    let d = crate::fit::Composer::of(m, form);
                    let s = |what: &str| -> String { format!("{what} {k}") };
                    let mark = |what: &str| {
                        let id = s(what);
                        div().debug_selector(move || id)
                    };
                    let body = |who: &str| {
                        [
                            mark(&format!("{who} text")).h(px(3. * m.body_line)),
                            mark(&format!("{who} foot")).mt(px(d.row_gap.unwrap_or(0.))).h(px(d.button)),
                        ]
                    };
                    let edges = |who: &str, at: f32| {
                        [
                            mark(&format!("{who} bar")).absolute().left(px(at)).top(px(at)).bottom(px(at)).w(px(2.)),
                            mark(&format!("{who} edge")).absolute().left(px(at)).right(px(at)).top(px(at)).h(px(1.)),
                            div()
                                .absolute()
                                .left(px(at))
                                .top(px(at))
                                .bottom(px(24. + at))
                                .w(px(2.))
                                .child(mark(&format!("{who} light")).absolute().left_0().top(relative(0.4)).w(px(2.)).h(px(24.))),
                            mark(&format!("{who} top")).absolute().inset_0(),
                        ]
                    };
                    // Main's (7135205): the frame, the padding in it, what
                    // is drawn over the frame a pixel out, the top's
                    // measure at its inset 0.
                    let old = v_flex()
                        .px(px(m.pad_x - 1.))
                        .pt(px(d.pad_top))
                        .pb(px(d.pad_bottom))
                        .relative()
                        .border_1()
                        .children(edges("old", -1.))
                        .children(body("old"));
                    let new = crate::app::composer_box(
                        window.scale_factor(),
                        (m.pad_x - 1., d.pad_top, d.pad_bottom),
                        edges("new", -1.).into_iter().chain(body("new")).map(IntoElement::into_any_element),
                    );
                    col = col
                        .child(old.debug_selector(move || format!("old box {k}")))
                        .child(new.debug_selector(move || format!("new box {k}")));
                }
                col
            }
        }
        let (view, cx) = cx.add_window_view(|_, _| Boxes(crate::theme::Metrics::at(crate::theme::DEFAULT_TEXT)));
        for scale in [1., 1.25, 1.5, 1.75, 2., 2.5, 0.5, 0.25] {
            cx.simulate_scale_factor_change(scale);
            view.update(cx, |_, cx| cx.notify());
            cx.run_until_parked();
            let dev = |v: Pixels| (f32::from(v) * scale).round() as i32;
            for (k, form) in FORMS.into_iter().enumerate() {
                // A part's offset in its box and its size, in device px.
                let mut at = |who: &str, what: &str| {
                    let b = cx.debug_bounds(format!("{who} box {k}").leak()).unwrap_or_else(|| panic!("{scale}: {who} box {k}"));
                    let e = cx.debug_bounds(format!("{who} {what} {k}").leak()).unwrap_or_else(|| panic!("{scale}: {who} {what} {k}"));
                    let (x0, y0) = (dev(b.origin.x), dev(b.origin.y));
                    [
                        dev(b.size.width),
                        dev(b.size.height),
                        dev(e.origin.x) - x0,
                        dev(e.origin.y) - y0,
                        dev(e.size.width),
                        dev(e.size.height),
                    ]
                };
                for what in ["text", "foot", "top", "bar", "edge", "light"] {
                    assert_eq!(at("new", what), at("old", what), "{scale} {form:?}: its {what}, as main's");
                }
            }
        }
    }

    #[test]
    fn verbs_are_plain_words() {
        assert_eq!(verb("bash").as_ref(), "Run");
        assert_eq!(verb("multi_edit").as_ref(), "Edit");
        assert_eq!(verb("grep").as_ref(), "Search");
        assert_eq!(verb("subagent_start").as_ref(), "Spawn");
        assert_eq!(verb("ext__github__search").as_ref(), "Tool");
        assert_eq!(verb("todo_write").as_ref(), "Todos");
        assert_eq!(verb("activate_tools").as_ref(), "Activate");
        assert_eq!(verb("load_skill").as_ref(), "Load");
        assert_eq!(verb("subagent_steer").as_ref(), "Steer");
    }

    #[test]
    fn a_failed_command_says_its_exit_code() {
        let e = |tool, text| Exit::of(tool, text);
        assert_eq!(e("bash", "Tool execution failed: Command failed (exit 1):\ncat: x"), Some(Exit::Code(1)));
        assert_eq!(e("bash", "Command failed (exit 101):\nerror: test failed"), Some(Exit::Code(101)));
        // A signal: the engine's -1, whatever the output under it says.
        assert_eq!(e("bash", "Command failed (exit -1):\nrunner: (exit status: 101)\nexit 1"), Some(Exit::Killed));
        // Only the engine's header, on the first line.
        assert_eq!(e("bash", "warning: x\nCommand failed (exit 2):"), None);
        assert_eq!(e("bash", "error: process exited with code 101"), None);
        // Only a shell command: another tool's error that says "exit".
        assert_eq!(e("ext__github__run", "Command failed (exit 3): the workflow"), None);
        assert_eq!(e("web_fetch", "exit 3"), None);
        assert_eq!(Exit::Code(2).word().as_ref(), "Exit 2");
        assert_eq!(Exit::Killed.word().as_ref(), "Killed");
    }

    #[test]
    fn durations_read_as_facts() {
        assert_eq!(took(Duration::from_millis(1400)).as_ref(), "1.4s");
        assert_eq!(took(Duration::from_secs(12)).as_ref(), "12s");
        assert_eq!(took(Duration::from_secs(83)).as_ref(), "1m 23s");
    }

    /// Allow's two styles both paint (the switch is one const in
    /// `authorize.rs`): the fill's label is `acc_ink` on the accent; the
    /// outline's the accent on the card, in `acc_line`. At rest, hovered and
    /// pressed, in every theme and accent preset (Shady #23 r2 L1); pressed
    /// is not rest.
    #[test]
    fn allow_paints_filled_or_outlined() {
        for (name, raw) in crate::palettes::THEMES {
            for accent in [None, Some(0xe8132f), Some(0xf4b400), Some(0x2d7bff), Some(0x35f2ff)] {
                let p = crate::theme::Palette::of(&crate::theme::resolve(&raw, accent).0);
                let lum = |c: gpui_kit::Hsla| crate::theme::luminance_hsla(c);
                let con = |a, b| {
                    let (x, y) = (lum(a), lum(b));
                    (x.max(y) + 0.05) / (x.min(y) + 0.05)
                };
                for (hovered, pressed) in [(false, false), (true, false), (true, true)] {
                    let (bg, _, text) = AllowStyle::Fill.colors(&p, hovered, pressed);
                    assert!(con(text, bg) >= 4.45, "{name} {accent:?}: Allow's label on its fill ({hovered}, {pressed})");
                    let (bg, edge, text) = AllowStyle::Outline.colors(&p, hovered, pressed);
                    assert_eq!(edge, p.acc_line);
                    assert!(con(text, bg) >= 4.45, "{name} {accent:?}: Allow's outlined label ({hovered}, {pressed})");
                }
                for style in [AllowStyle::Fill, AllowStyle::Outline] {
                    assert_ne!(
                        style.colors(&p, true, true).0,
                        style.colors(&p, false, false).0,
                        "{name} {accent:?}: {style:?} pressed shows"
                    );
                }
            }
        }
    }
}
