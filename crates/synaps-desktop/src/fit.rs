//! The centre column's vertical budget: what the chrome around the
//! transcript takes at a text step, in each form, and the window heights
//! where the form changes. The card and the composer are drawn with these
//! sizes, so the thresholds hold for what is on screen.
//!
//! - **Full:** the 52-px top bar, the two-line session header, a well of
//!   up to 12 lines on the authorization card.
//! - **Short** ([`short_below`]): a 40-px top bar, the one-row header, a
//!   well of up to 8 lines, the card's lead on one line, less padding
//!   round the composer.
//! - **Cramped** ([`cramped_below`]): short, and the card folds its lead
//!   into its title row and pads less; the composer puts SEND beside the
//!   text. A 3-line well, AUTHORIZE and the composer fit a 420-px window
//!   (the smallest) at every text step.
//!
//! In every form the well takes what is left, between [`WELL_MIN`] lines
//! and the form's most; the transcript's list gives way first. What else
//! the column may draw ([`Extras`]: the naming row, the notice line)
//! counts too: it moves the cramped line up, and in a cramped window the
//! well gives way to it down to one line, so SEND stays on screen in a
//! 420-px window at every text step with all of it open.

use crate::chrome::{TOP, TOP_SHORT};
use crate::tabstrip::TAB_H;
use crate::theme::{DEFAULT_TEXT, Metrics};

/// The fewest lines the card's well shows: there is always something to
/// read (the rest scrolls, and AUTHORIZE waits for its end).
pub(crate) const WELL_MIN: f32 = 3.;
/// The most it shows in a tall window...
pub(crate) const WELL_MAX: f32 = 12.;
/// ...and in a short one. A typical confirm (a one- or two-line guard
/// message, no rewrite) fits whole; a longer one scrolls.
pub(crate) const WELL_MAX_SHORT: f32 = 8.;
/// The composer's text grows to this many rows, fewer when the window has
/// no room for them under the card ([`composer_rows`]).
pub(crate) const COMPOSER_ROWS: usize = 6;
/// The session header, its hairline included: two lines, and one row.
pub(crate) const HEADER_FULL: f32 = 92.;
pub(crate) const HEADER_SHORT: f32 = 37.;
/// What a full window keeps for the transcript under a 12-line card (at
/// 16 px; scaled with the text).
const LIST_ROOM: f32 = 55.;
/// A Medium input's padding above and below its text (`input_py`).
const FIELD_PAD: f32 = 8.;
/// GPUI's default line height, in ems: a row of text left at its natural
/// height (the card's title) is this tall.
const PHI: f32 = std::f32::consts::GOLDEN_RATIO;
/// The naming row's field (a Medium input, 2 rem).
const NAME_FIELD: f32 = 32.;
/// The notice line's gap above its text.
pub(crate) const NOTICE_TOP: f32 = 6.;

/// The naming row's padding above and below (less when cramped).
pub(crate) fn naming_pad(form: Form) -> f32 {
    if form == Form::Cramped { 4. } else { 10. }
}

/// The naming row, its hairline included.
pub(crate) fn naming_h(form: Form) -> f32 {
    NAME_FIELD + 2. * naming_pad(form) + 1.
}

/// The notice line's line (one line while a card is up: the rest is in
/// its tooltip).
pub(crate) fn notice_line(m: &Metrics) -> f32 {
    (18. * m.r).round()
}

/// What else the centre column draws round the card and the composer:
/// the naming row ("Name this session…", its own error or REPLACE line
/// not counted) and the notice line under the card: the last error and,
/// cramped, the draft's note (the composer keeps SEND beside its text).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Extras {
    pub(crate) naming: bool,
    pub(crate) error: bool,
    pub(crate) note: bool,
}

impl Extras {
    /// The notice line is drawn in `form`.
    pub(crate) fn notice(&self, form: Form) -> bool {
        self.error || (self.note && form == Form::Cramped)
    }

    pub(crate) fn height(&self, m: &Metrics, form: Form) -> f32 {
        let naming = if self.naming { naming_h(form) } else { 0. };
        naming + if self.notice(form) { NOTICE_TOP + notice_line(m) } else { 0. }
    }

    /// The fewest lines the well keeps in `form`: one when they take room
    /// in a cramped window, else [`WELL_MIN`].
    pub(crate) fn well_min(&self, m: &Metrics, form: Form) -> f32 {
        if form == Form::Cramped && self.height(m, form) > 0. { 1. } else { WELL_MIN }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Form {
    Full,
    Short,
    Cramped,
}

/// A filled button's height (AUTHORIZE, SEND).
pub(crate) fn button_h(m: &Metrics) -> f32 {
    (30. * m.r).round().max(28.)
}

/// The authorization card's boxes, top to bottom.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Card {
    /// Between the list and the card.
    pub(crate) top: f32,
    /// Inside its border, above and below.
    pub(crate) pad_y: f32,
    /// The title row (its natural height, but cramped: set).
    pub(crate) title: f32,
    /// The lead's gap and line; `None`: folded into the title row.
    pub(crate) lead: Option<(f32, f32)>,
    pub(crate) well_gap: f32,
    pub(crate) well_pad: f32,
    pub(crate) line: f32,
    /// The most lines the well shows.
    pub(crate) lines: f32,
    pub(crate) actions_gap: f32,
    pub(crate) button: f32,
}

impl Card {
    pub(crate) fn of(m: &Metrics, form: Form) -> Self {
        let px = |v: f32| (v * m.r).round();
        let cramped = form == Form::Cramped;
        Card {
            top: if cramped { 0. } else { px(6.) },
            pad_y: px(if cramped { 6. } else { 16. }),
            title: if cramped { px(22.) } else { m.card_title * PHI },
            lead: (!cramped).then(|| (px(8.), m.card_lead_line)),
            well_gap: px(if cramped { 6. } else { 10. }),
            well_pad: px(if cramped { 6. } else { 10. }),
            line: m.code_line,
            lines: if form == Form::Full { WELL_MAX } else { WELL_MAX_SHORT },
            actions_gap: px(if cramped { 8. } else { 14. }),
            button: button_h(m),
        }
    }

    /// The well, its padding included, showing `lines`.
    pub(crate) fn well(&self, lines: f32) -> f32 {
        lines * self.line + 2. * self.well_pad
    }

    /// The card's box (border included, the gap above it not) with a
    /// one-line lead and `lines` in the well.
    pub(crate) fn boxed(&self, lines: f32) -> f32 {
        2. + 2. * self.pad_y
            + self.title
            + self.lead.map_or(0., |(gap, line)| gap + line)
            + self.well_gap
            + self.well(lines)
            + self.actions_gap
            + self.button
    }

    /// What it takes in the column: the gap above and the box.
    pub(crate) fn height(&self, lines: f32) -> f32 {
        self.top + self.boxed(lines)
    }
}

/// The composer's boxes, top to bottom.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Composer {
    /// Above its box.
    pub(crate) top: f32,
    /// Inside the box, above the text and below the last row.
    pub(crate) pad_top: f32,
    pub(crate) pad_bottom: f32,
    /// The gap above the row with SEND; `None`: SEND beside the text.
    pub(crate) row_gap: Option<f32>,
    pub(crate) button: f32,
    pub(crate) line: f32,
    /// Under its box.
    pub(crate) bottom: f32,
}

impl Composer {
    pub(crate) fn of(m: &Metrics, form: Form) -> Self {
        let px = |v: f32| (v * m.r).round();
        let base = Composer {
            top: px(10.),
            pad_top: m.pad_y,
            pad_bottom: px(11.),
            row_gap: Some(px(10.)),
            button: button_h(m),
            line: m.body_line,
            bottom: px(22.),
        };
        match form {
            Form::Full => base,
            Form::Short => Composer { top: px(6.), bottom: px(12.), ..base },
            Form::Cramped => Composer { top: px(4.), pad_top: px(6.), pad_bottom: px(6.), row_gap: None, bottom: px(6.), ..base },
        }
    }

    /// The text field with `rows` lines.
    pub(crate) fn field(&self, rows: usize) -> f32 {
        rows as f32 * self.line + 2. * FIELD_PAD
    }

    /// What it takes in the column with `rows` lines of text.
    pub(crate) fn height(&self, rows: usize) -> f32 {
        let inner = match self.row_gap {
            Some(gap) => self.field(rows) + gap + self.button,
            None => self.field(rows).max(self.button),
        };
        self.top + 2. + self.pad_top + inner + self.pad_bottom + self.bottom
    }
}

/// The top bar and the session header in `form`.
pub(crate) fn above(form: Form) -> f32 {
    match form {
        Form::Full => TOP + TAB_H + HEADER_FULL,
        _ => TOP_SHORT + TAB_H + HEADER_SHORT,
    }
}

/// The window height `form` needs at `m`: the bars, the header, `ex`, a
/// card with its least well ([`Extras::well_min`]) and a one-line
/// composer (no transcript).
pub(crate) fn need(m: &Metrics, form: Form, ex: Extras) -> f32 {
    above(form) + ex.height(m, form) + Card::of(m, form).height(ex.well_min(m, form)) + Composer::of(m, form).height(1)
}

/// The centre pane's height (under the full top bar) below which the
/// window is short: the full chrome, a 12-line card and the composer would
/// leave the transcript under 55 px. 740 at 16 px (a 792-px window: every
/// 800-px-class laptop screen is short, a 900-px one is not); larger with
/// larger text, and never smaller than at 16 px.
pub(crate) fn short_below(m: &Metrics) -> f32 {
    let m = if m.r < 1. { Metrics::at(DEFAULT_TEXT) } else { *m };
    (TAB_H + HEADER_FULL + Card::of(&m, Form::Full).height(WELL_MAX) + Composer::of(&m, Form::Full).height(1) + (LIST_ROOM * m.r).round())
        .floor()
}

/// ...below which it is cramped: the short form would not fit a 3-line
/// well, `ex` and a one-line composer.
pub(crate) fn cramped_below(m: &Metrics, ex: Extras) -> f32 {
    need(m, Form::Short, ex) - TOP
}

/// The fewest lines the well shows: `floor` (the body's own lines, up to
/// [`WELL_MIN`]), or fewer if `ex` needs the room ([`Extras::well_min`]).
pub(crate) fn well_floor(m: &Metrics, form: Form, floor: f32, ex: Extras) -> f32 {
    floor.min(ex.well_min(m, form))
}

/// The lines the card's well shows in a `window` this tall: whole lines,
/// as many as the bars, `ex`, the card's other boxes and a composer of
/// `rows` leave room for, between [`well_floor`] and the form's most.
/// (What the budget does not count, the naming row's own error line, the
/// layout takes from the well, down to that floor.)
pub(crate) fn well_lines(m: &Metrics, form: Form, window: f32, rows: usize, floor: f32, ex: Extras) -> f32 {
    let card = Card::of(m, form);
    let room = window - above(form) - ex.height(m, form) - Composer::of(m, form).height(rows) - card.height(0.);
    (room / card.line).floor().clamp(well_floor(m, form, floor, ex), card.lines)
}

/// How many rows the composer's text may grow to: as many as fit under
/// a card with a 3-line well (`card`: one is up) and `ex`, at least one.
pub(crate) fn composer_rows(m: &Metrics, form: Form, window: f32, card: bool, ex: Extras) -> usize {
    let c = Composer::of(m, form);
    let card = if card { Card::of(m, form).height(WELL_MIN) } else { 0. };
    let spare = window - above(form) - ex.height(m, form) - card - c.height(1);
    (1 + (spare / c.line).floor().max(0.) as usize).min(COMPOSER_ROWS)
}

/// The top bar's fixed words, as wide as they are drawn ([`TopWords`] is
/// measured once, in the embedded faces).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TopWords {
    /// SYNAPS and DESKTOP (display, 13 px).
    pub(crate) word: f32,
    pub(crate) word2: f32,
    /// "   NEED YOU" after the count, and one digit of it (display, 11 px).
    pub(crate) need: f32,
    pub(crate) digit: f32,
    /// JUMP and THEME (display, 10.5 px), SYSTEMS (11 px).
    pub(crate) jump: f32,
    pub(crate) theme: f32,
    pub(crate) systems: f32,
    /// A mono cell at 11 px (the ticker) and at 10.5 / 10 px (the
    /// connection, the SYSTEMS badge).
    pub(crate) cell: f32,
    pub(crate) cell_conn: f32,
    pub(crate) cell_badge: f32,
}

/// What the top bar has to show this frame: the window's width, the brand
/// block's width over an open rail, and how long each changing text is.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct TopIn {
    pub(crate) width: f32,
    pub(crate) brand: f32,
    /// The attention count, the session label's and the gist's characters,
    /// the "+N"'s digits (0: none), JUMP.
    pub(crate) need: usize,
    pub(crate) label: usize,
    pub(crate) gist: usize,
    pub(crate) more: usize,
    pub(crate) jump: bool,
    /// The connection text's characters, the SYSTEMS badge's digits.
    pub(crate) conn: usize,
    pub(crate) badge: usize,
}

/// What the top bar draws ([`fit_top`]): each text's width (0: not drawn),
/// each fixed word on or off.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TopFit {
    pub(crate) brand: f32,
    pub(crate) word: bool,
    pub(crate) word2: bool,
    pub(crate) label: f32,
    pub(crate) gist: f32,
    pub(crate) conn: f32,
    pub(crate) theme: bool,
    pub(crate) systems: bool,
}

/// The bar's padding (22 left, 24 right) and some slack, and the least
/// space each side of the ticker.
const TOP_PAD: f32 = 52.;
const TOP_SPACE: f32 = 16.;
/// Shortened (ellipsized) no narrower than this before it goes.
const GIST_MIN: f32 = 96.;
const CONN_MIN: f32 = 72.;
const LABEL_MIN: f32 = 60.;
/// As the mock caps them.
pub(crate) const GIST_MAX: f32 = 330.;
pub(crate) const CONN_MAX: f32 = 260.;

fn digits(n: usize) -> usize {
    n.max(1).ilog10() as usize + 1
}

impl TopFit {
    /// Every part's width plus the least space between them.
    fn width(&self, w: &TopWords, i: &TopIn) -> f32 {
        let on = |b: bool, v: f32| if b { v } else { 0. };
        let text = |v: f32, gap: f32| if v > 0. { v + gap } else { 0. };
        let ticker = 6.
            + 12.
            + w.need
            + digits(i.need) as f32 * w.digit
            + text(self.label, 12.)
            + text(self.gist, 12.)
            + on(i.more > 0, 12. + (1 + i.more) as f32 * w.cell)
            + on(i.jump, 12. + 8. + w.jump + 6. + w.cell);
        let badge = (10. + i.badge as f32 * w.cell_badge).max(18.);
        let right = text(self.conn, 18.) + 10. + on(self.theme, 8. + w.theme) + 18. + on(self.systems, w.systems + 10.) + badge;
        self.brand + TOP_SPACE + ticker + TOP_SPACE + right
    }

    fn brand_content(&self, w: &TopWords) -> f32 {
        8. + if self.word { 10. + w.word } else { 0. } + if self.word2 { 14. + w.word2 } else { 0. }
    }
}

/// The top bar fitted to the window: as it narrows, these give way in
/// this order, each only as far as needed: the brand block stops lining up
/// with the rail; DESKTOP; the prompt's gist and the connection text
/// shorten (ellipsized); the gist goes; the session label shortens; the
/// THEME and SYSTEMS words go (the swatch and the count stay); the
/// connection text goes (the disc keeps its colour); SYNAPS; the label.
/// The count, NEED YOU, "+N", JUMP, the swatch and the badge always show.
/// Then what a later step freed is given back, back up the order: a word
/// that fits again comes back, a shortened text grows into what is left.
pub(crate) fn fit_top(w: &TopWords, i: &TopIn) -> TopFit {
    let room = i.width - TOP_PAD;
    let mut f = TopFit {
        brand: i.brand,
        word: true,
        word2: true,
        label: i.label as f32 * w.cell,
        gist: (i.gist as f32 * w.cell).min(GIST_MAX),
        conn: (i.conn as f32 * w.cell_conn).min(CONN_MAX),
        theme: true,
        systems: true,
    };
    f.brand = f.brand.max(f.brand_content(w));
    type Step = fn(&mut TopFit, &TopWords, f32);
    fn shrink(v: &mut f32, min: f32, over: f32) {
        if *v > min {
            *v = (*v - over).max(min);
        }
    }
    let steps: [Step; 11] = [
        |f, w, _| f.brand = f.brand_content(w),
        |f, w, _| {
            f.word2 = false;
            f.brand = f.brand_content(w);
        },
        |f, _, over| shrink(&mut f.gist, GIST_MIN, over),
        |f, _, over| shrink(&mut f.conn, CONN_MIN, over),
        |f, _, _| f.gist = 0.,
        |f, _, over| shrink(&mut f.label, LABEL_MIN, over),
        |f, _, _| f.theme = false,
        |f, _, _| f.systems = false,
        |f, _, _| f.conn = 0.,
        |f, w, _| {
            f.word = false;
            f.brand = f.brand_content(w);
        },
        |f, _, _| f.label = 0.,
    ];
    // Each step taken: what it changed from.
    let mut taken: [Option<TopFit>; 11] = [None; 11];
    for (k, step) in steps.iter().enumerate() {
        let over = f.width(w, i) - room;
        if over <= 0. {
            break;
        }
        let before = f;
        step(&mut f, w, over);
        if f != before {
            taken[k] = Some(before);
        }
    }
    for (k, before) in taken.iter().enumerate().rev().filter_map(|(k, b)| Some((k, (*b)?))) {
        let mut g = f;
        match k {
            0 => g.brand = before.brand.max(g.brand_content(w)),
            1 => {
                g.word2 = true;
                g.brand = g.brand.max(g.brand_content(w));
            }
            9 => {
                g.word = true;
                g.brand = g.brand.max(g.brand_content(w));
            }
            6 => g.theme = true,
            7 => g.systems = true,
            _ => {
                let (slot, min): (fn(&mut TopFit) -> &mut f32, f32) = match k {
                    2 | 4 => (|t| &mut t.gist, GIST_MIN),
                    3 | 8 => (|t| &mut t.conn, CONN_MIN),
                    _ => (|t| &mut t.label, LABEL_MIN),
                };
                // As it was before the step, or shortened to what fits:
                // longer than now, and (back from gone) not under its floor.
                let mut b = before;
                let (was, now) = (*slot(&mut b), *slot(&mut g));
                *slot(&mut g) = was;
                let v = was - (g.width(w, i) - room).max(0.);
                *slot(&mut g) = if v > now && (now > 0. || v >= min.min(was)) { v } else { now };
            }
        }
        if g.width(w, i) <= room + 0.01 {
            f = g;
        }
    }
    f
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::TEXT_STEPS;
    use crate::turns::Layout;

    fn form(l: Layout) -> Form {
        l.form()
    }

    /// The 16-px figures the short window was introduced with.
    #[test]
    fn the_16_px_budget() {
        let m = Metrics::at(16);
        assert_eq!(short_below(&m), 740.);
        // The card's box with 8 lines: 330 px and the title's fraction (as drawn).
        assert_eq!(Card::of(&m, Form::Full).boxed(8.).floor(), 330.);
        assert_eq!(Composer::of(&m, Form::Full).height(1), 139.);
        assert_eq!(Composer::of(&m, Form::Short).height(1), 125.);
        assert_eq!(cramped_below(&m, Extras::default()).floor(), 415.);
        // Cramped at 22 px: 406 of the smallest window's 420.
        assert_eq!(need(&Metrics::at(22), Form::Cramped, Extras::default()), 406.);
        // Smaller text never makes a window less short than at 16 px.
        assert_eq!(short_below(&Metrics::at(14)), 740.);
    }

    /// M1: at every text step, every window height from the smallest up:
    /// the form chosen fits a 3-line well, AUTHORIZE and a one-line
    /// composer, and the thresholds grow with the text.
    #[test]
    fn the_card_and_composer_fit_down_to_the_smallest_window() {
        let mut last = (0., 0.);
        for step in TEXT_STEPS {
            let m = Metrics::at(step);
            let (short, cramped) = (short_below(&m), cramped_below(&m, Extras::default()));
            assert!(cramped < short, "{step}");
            assert!(short >= last.0 && cramped >= last.1, "{step}");
            last = (short, cramped);
            for window in 420..1600 {
                let window = window as f32;
                let f = form(Layout::of(1000., window - TOP, &m, Extras::default()));
                assert!(
                    need(&m, f, Extras::default()) <= window,
                    "step {step} window {window}: {f:?} needs {}",
                    need(&m, f, Extras::default())
                );
                // Full only with room for 12 lines and some transcript.
                if f == Form::Full {
                    assert!(above(f) + Card::of(&m, f).height(WELL_MAX) + Composer::of(&m, f).height(1) < window);
                }
                assert!(composer_rows(&m, f, window, true, Extras::default()) >= 1);
            }
        }
        // Step 22: full from 1018 px (792 at 16).
        let m = Metrics::at(22);
        assert_eq!(short_below(&m) + TOP, 1018.);
        assert_eq!(form(Layout::of(1000., 1018. - TOP, &m, Extras::default())), Form::Full);
        assert_eq!(form(Layout::of(1000., 1017. - TOP, &m, Extras::default())), Form::Short);
        assert_eq!(form(Layout::of(1000., 420. - TOP, &m, Extras::default())), Form::Cramped);
    }

    /// The well shows whole lines: 7 at 16 px in the smallest window, 3 at
    /// 22 px; 8 in a 600-px window (short), 12 in a full one.
    #[test]
    fn the_well_shows_whole_lines() {
        let m = Metrics::at(16);
        assert_eq!(well_lines(&m, Form::Cramped, 420., 1, 3., Extras::default()), 7.);
        assert_eq!(well_lines(&m, Form::Short, 600., 1, 3., Extras::default()), 8.);
        assert_eq!(well_lines(&m, Form::Full, 800., 1, 3., Extras::default()), 12.);
        // A taller draft takes its rows from the well.
        assert_eq!(well_lines(&m, Form::Cramped, 420., 3, 3., Extras::default()), 4.);
        let m = Metrics::at(22);
        assert_eq!(well_lines(&m, Form::Cramped, 420., 1, 3., Extras::default()), 3.);
        // Never under the floor (the layout clips, the well scrolls).
        assert_eq!(well_lines(&m, Form::Cramped, 300., 1, 3., Extras::default()), 3.);
        assert_eq!(well_lines(&m, Form::Cramped, 300., 1, 2., Extras::default()), 2.);
    }

    /// Every mix of the naming row, an error and a draft note.
    fn all_extras() -> impl Iterator<Item = Extras> {
        (0..8u8).map(|b| Extras { naming: b & 1 != 0, error: b & 2 != 0, note: b & 4 != 0 })
    }

    /// L1: with the naming row, the notice line (an error, cramped the
    /// draft note) or all of them open, at every text step and every
    /// window height from the smallest up, the form chosen fits its well's
    /// floor, AUTHORIZE and a one-line composer: SEND is on screen. The
    /// well keeps whole lines and gives way to them only when cramped.
    #[test]
    fn the_naming_row_and_the_notice_line_are_counted() {
        for step in TEXT_STEPS {
            let m = Metrics::at(step);
            for ex in all_extras() {
                for window in 420..1600 {
                    let window = window as f32;
                    let f = form(Layout::of(1000., window - TOP, &m, ex));
                    let need = need(&m, f, ex);
                    assert!(need <= window, "step {step} window {window} {ex:?}: {f:?} needs {need}");
                    let lines = well_lines(&m, f, window, 1, WELL_MIN, ex);
                    assert_eq!(lines, lines.floor());
                    assert!(lines >= ex.well_min(&m, f));
                    let drawn = above(f) + ex.height(&m, f) + Card::of(&m, f).height(lines) + Composer::of(&m, f).height(1);
                    assert!(drawn <= window + 0.01, "step {step} window {window} {ex:?}: {drawn}");
                    if f != Form::Cramped {
                        assert_eq!(ex.well_min(&m, f), WELL_MIN);
                    }
                }
                // The thresholds only move up with more open.
                assert!(cramped_below(&m, ex) >= cramped_below(&m, Extras::default()));
            }
        }
        // Step 22, the smallest window, everything open: 420 of 420.
        let (m, ex) = (Metrics::at(22), Extras { naming: true, error: true, note: true });
        assert_eq!(need(&m, Form::Cramped, ex), 420.);
        assert_eq!(well_lines(&m, Form::Cramped, 420., 1, WELL_MIN, ex), 1.);
        // The note is on the notice line only when cramped (else in the
        // composer's own row); the error always.
        let note = Extras { note: true, ..Extras::default() };
        assert_eq!((note.notice(Form::Cramped), note.notice(Form::Short)), (true, false));
        assert_eq!(note.height(&m, Form::Short), 0.);
        assert_eq!(naming_h(Form::Cramped), 41.);
        assert_eq!(naming_h(Form::Short), 53.);
    }

    /// The composer grows only into room the card's 3-line form leaves.
    #[test]
    fn the_composer_grows_into_spare_room_only() {
        let m = Metrics::at(22);
        assert_eq!(composer_rows(&m, Form::Cramped, 420., true, Extras::default()), 1);
        assert_eq!(composer_rows(&m, Form::Full, 1400., true, Extras::default()), COMPOSER_ROWS);
        let m = Metrics::at(16);
        let rows = composer_rows(&m, Form::Cramped, 420., true, Extras::default());
        assert!(rows > 1);
        let c = Composer::of(&m, Form::Cramped);
        assert!(above(Form::Cramped) + Card::of(&m, Form::Cramped).height(WELL_MIN) + c.height(rows) <= 420.);
        assert!(above(Form::Cramped) + Card::of(&m, Form::Cramped).height(WELL_MIN) + c.height(rows + 1) > 420.);
    }

    /// The words as the embedded faces draw them (measured on Linux).
    pub(crate) const WORDS: TopWords = TopWords {
        word: 58.,
        word2: 66.,
        need: 70.,
        digit: 6.,
        jump: 32.,
        theme: 38.,
        systems: 58.,
        cell: 6.6,
        cell_conn: 6.3,
        cell_badge: 6.,
    };

    /// The top bar never overflows, from the smallest window up; what it
    /// drops goes in its order, and what a later drop frees is given back.
    #[test]
    fn the_top_bar_degrades_in_order() {
        let busy = TopIn { brand: 260., need: 12, label: 24, gist: 60, more: 2, jump: true, conn: 60, badge: 2, ..TopIn::default() };
        let calm = TopIn { brand: 254., need: 0, label: 0, gist: 0, more: 0, jump: false, conn: 18, badge: 1, ..TopIn::default() };
        for i in [busy, calm] {
            let mut wider: Option<TopFit> = None;
            for width in (640..2600).rev() {
                let i = TopIn { width: width as f32, ..i };
                let f = fit_top(&WORDS, &i);
                assert!(f.width(&WORDS, &i) <= i.width - TOP_PAD + 0.01, "{width}: {f:?}");
                // A text may grow a pixel narrower (the drop of another freed
                // room, L3); SYNAPS never comes back.
                if let Some(o) = wider {
                    assert!(!f.word || o.word, "{width}");
                }
                // L3: nothing taken would fit again: a shortened text has
                // grown into what was left, a word gone does not fit.
                let left = i.width - TOP_PAD - f.width(&WORDS, &i);
                let whole = fit_top(&WORDS, &TopIn { width: 1e5, ..i });
                for (v, all) in [(f.gist, whole.gist), (f.conn, whole.conn), (f.label, whole.label)] {
                    assert!(!(v > 0. && v < all) || left < 0.02, "{width}: {left} px left with a text shortened: {f:?}");
                }
                for (on, all, cost) in [
                    (f.theme, whole.theme, 8. + WORDS.theme),
                    (f.systems, whole.systems, WORDS.systems + 10.),
                    (f.word2, whole.word2, 14. + WORDS.word2),
                ] {
                    assert!(on || !all || left < cost, "{width}: {left} px left, a word of {cost} gone: {f:?}");
                }
                // Shortened never under its floor: whole, at least the
                // floor, or not drawn.
                let whole = fit_top(&WORDS, &TopIn { width: 1e5, ..i });
                for (v, all, min) in [(f.gist, whole.gist, GIST_MIN), (f.conn, whole.conn, CONN_MIN), (f.label, whole.label, LABEL_MIN)] {
                    assert!(v == 0. || v == all || v >= min, "{width}: {f:?}");
                }
                wider = Some(f);
            }
        }
        // Wide: everything, over the rail. 860 busy: DESKTOP and the gist
        // went, THEME and SYSTEMS are still there.
        let f = fit_top(&WORDS, &TopIn { width: 2560., ..busy });
        assert_eq!((f.brand, f.word2, f.theme, f.systems), (260., true, true, true));
        assert!(f.gist > 0. && f.conn > 0. && f.label > 0.);
        let f = fit_top(&WORDS, &TopIn { width: 860., ..busy });
        assert!(!f.word2 && f.gist == 0. && f.word);
        // The smallest window keeps the count, JUMP, the swatch and the badge (in `width`).
        let f = fit_top(&WORDS, &TopIn { width: 640., ..busy });
        assert!(f.width(&WORDS, &TopIn { width: 640., ..busy }) <= 640. - TOP_PAD);
        // L3, the review's cases: at 888 the connection text is not cut to
        // its floor with 107 px to spare; at 640 with 123 waiting and a
        // 3-digit badge, dropping SYSTEMS left 59 px, and THEME (46) is back.
        let i = TopIn { width: 888., ..busy };
        let f = fit_top(&WORDS, &i);
        assert!(f.conn > CONN_MIN && i.width - TOP_PAD - f.width(&WORDS, &i) < 0.02, "{f:?}");
        let i = TopIn { width: 640., need: 123, badge: 3, ..busy };
        let f = fit_top(&WORDS, &i);
        assert!(f.theme && !f.systems, "{f:?}");
    }
}
