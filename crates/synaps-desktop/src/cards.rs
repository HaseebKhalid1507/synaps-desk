//! Cards D (cards4/proposal.md §1–§4): a run of tool calls is one
//! square card (Haseeb, #23 r7) on `card_d` in a 1 px `line` hairline; each call is
//! one quiet 34 px row inside it (a life glyph, the verb in Sans, the
//! target in mono, the facts right in small grey Sans, a state word, the
//! chevron); its body sits in a `well_d` lined up under the verb. The list
//! is virtualized, so a run's card is drawn by its rows: the first carries
//! its top edge and corners, the last its bottom ([`edges`]).

use std::time::Duration;

use gpui_kit::component::h_flex;
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
    pub row: f32,
    pub pad: f32,
    pub gap: f32,
    pub glyph: f32,
    pub verb: f32,
    /// A body's indent from the row's edge: the verb column.
    pub indent: f32,
    pub chevron: f32,
    /// The corners of a card (a run's, the authorize card): square, as
    /// every radius here (Haseeb, #23 r7). Buttons, the checkbox, dots and
    /// the lattice keep their own.
    pub radius: f32,
    /// A row's inside a card (its hover's wash).
    pub row_radius: f32,
    /// A well's: a call's body (its diff, its output), the authorize
    /// card's command box.
    pub well_radius: f32,
}

impl Geo {
    pub(crate) fn of(m: &Metrics) -> Self {
        let px = |v: f32| (v * m.r).round();
        let (pad, gap, glyph) = (px(10.), px(10.), crate::glyph::LIFE);
        Geo {
            inset: 3.,
            row: px(34.),
            pad,
            gap,
            glyph,
            verb: px(56.),
            indent: pad + glyph + gap,
            chevron: 10.,
            radius: 0.,
            row_radius: 0.,
            well_radius: 0.,
        }
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

/// A card's slice for one list row (§3): `card_d` with the `line` hairline
/// down both sides, its top edge and corners on its first row, its bottom
/// on its last, and the 3 px inner padding on those.
pub(crate) fn slab(top: bool, bottom: bool, body: impl IntoElement, p: &Palette, g: Geo) -> Div {
    let r = px(g.radius);
    div()
        .w_full()
        .bg(p.card_d)
        .border_color(p.line)
        .border_l_1()
        .border_r_1()
        .px(px(g.inset))
        .when(top, |d| d.border_t_1().rounded_tl(r).rounded_tr(r).pt(px(g.inset)))
        .when(bottom, |d| d.border_b_1().rounded_bl(r).rounded_br(r).pb(px(g.inset)))
        .child(body)
}

/// A fact's separator (§3): a 3 px dot in `dim` at 55 %, centred.
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
