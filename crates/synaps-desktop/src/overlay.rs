//! The one overlay box (design.md §1.4, §3, §8.7–8.9): `card` with a 1 px
//! `frame`, square, no shadow (the kit's tooltip draws one: it is taken
//! off). Tooltips, the hover card, the rail readout and the menus are
//! drawn on it, and their key/value tables are [`facts`].

use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::app::display;
use crate::fonts::{self, size};
use crate::theme::{self, Palette};

/// A fact's key column (E L A P S E D at 12 px is 60.2 px).
pub(crate) const KEY_W: f32 = 72.;

/// The box, on any element: `card`, a 1 px `frame`.
pub(crate) fn frame<E: Styled>(el: E, p: &Palette) -> E {
    el.bg(p.card).border_1().border_color(p.frame)
}

/// A tooltip: `text` in `s.title` `prose_k`, padded 8 / 4.
pub(crate) fn tip(text: impl Into<SharedString>, window: &mut Window, cx: &mut App) -> AnyView {
    boxed(Tooltip::new(text.into()), cx).build(window, cx)
}

/// [`tip`], with the key bound to `action` after the text.
pub(crate) fn tip_key(text: impl Into<SharedString>, action: &dyn Action, window: &mut Window, cx: &mut App) -> AnyView {
    boxed(Tooltip::new(text.into()).action(action, None), cx).build(window, cx)
}

/// A body of its own on the box (the hover card): the kit's padding off,
/// the body pads itself.
pub(crate) fn element<E: IntoElement>(body: impl Fn(&mut Window, &mut App) -> E + 'static, window: &mut Window, cx: &mut App) -> AnyView {
    let p = theme::palette(cx);
    Tooltip::element(body).bg(p.card).border_color(p.frame).p_0().shadow_none().build(window, cx)
}

fn boxed(t: Tooltip, cx: &App) -> Tooltip {
    let p = theme::palette(cx);
    t.bg(p.card)
        .border_color(p.frame)
        .shadow_none()
        .px(px(8.))
        .py(px(4.))
        .font_family(fonts::SANS)
        .text_size(px(size::TITLE))
        .line_height(px(18.))
        .text_color(p.prose_k)
}

/// One row of a key/value table: its key (tracked caps), its value, and
/// a second, quieter line under the value (empty: none).
pub(crate) struct Fact {
    pub key: &'static str,
    pub value: SharedString,
    pub aside: SharedString,
}

impl Fact {
    pub fn new(key: &'static str, value: impl Into<SharedString>) -> Self {
        Fact { key, value: value.into(), aside: SharedString::default() }
    }

    pub fn aside(mut self, aside: impl Into<SharedString>) -> Self {
        self.aside = aside.into();
        self
    }
}

/// The key/value table: keys `d.s` `dim_k` in a 72 px column, values
/// `m.meta` `prose_k` (an aside under it in `dim_k`), 4 between rows.
pub(crate) fn facts(rows: impl IntoIterator<Item = Fact>, p: &Palette) -> Div {
    v_flex().gap(px(4.)).children(rows.into_iter().map(|f| {
        h_flex().items_start().child(div().w(px(KEY_W)).flex_none().pt(px(1.)).child(display(f.key, size::D_S, p.dim_k))).child(
            v_flex()
                .flex_1()
                .min_w_0()
                .font_family(fonts::MONO)
                .text_size(px(size::META))
                .line_height(px(18.))
                .child(div().text_color(p.prose_k).child(f.value))
                .when(!f.aside.is_empty(), |d| d.child(div().text_color(p.dim_k).child(f.aside))),
        )
    }))
}
