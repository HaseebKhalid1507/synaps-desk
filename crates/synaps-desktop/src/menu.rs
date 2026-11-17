//! A context menu: a column of items in the gear menu's style, at the
//! pointer (a right-click) or over a row (the Menu key, Shift+F10). ↑/↓,
//! Tab/Shift+Tab (Home/End) move the selection over the items that can
//! act, Enter or Space picks (a new press: a held key's repeats never do),
//! Esc or a click elsewhere closes it. Nothing moves. No GPUI in the logic
//! ([`Menu`]); [`render`] draws it.

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::app::{AppView, mono};
use crate::fonts::size;
use crate::theme::Palette;

/// One item: what it does, what it says (and an emphasised word after,
/// e.g. CONFIRM), its key, and whether it can act now (else why not).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Item<A> {
    pub act: A,
    pub label: SharedString,
    pub confirm: Option<SharedString>,
    pub key: Option<&'static str>,
    pub why_not: Option<SharedString>,
}

impl<A> Item<A> {
    pub fn new(act: A, label: impl Into<SharedString>) -> Self {
        Item { act, label: label.into(), confirm: None, key: None, why_not: None }
    }

    pub fn key(mut self, key: &'static str) -> Self {
        self.key = Some(key);
        self
    }

    /// It cannot act now: `why` (its tooltip).
    pub fn off(mut self, why: impl Into<SharedString>) -> Self {
        self.why_not = Some(why.into());
        self
    }

    pub fn enabled(&self) -> bool {
        self.why_not.is_none()
    }
}

/// Where the menu is: at a point in the window (a right-click), or over
/// the row it is for (opened from the keyboard; the row draws it).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum At {
    Point(Point<Pixels>),
    Row(u64),
}

/// How an item was picked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum How {
    /// A click, its count (2: the second click of a double-click).
    Click(usize),
    /// Enter or Space; `press`: a new press, not a held key's repeat.
    Key { press: bool },
}

impl How {
    /// A new press: a single click, or a key's first key-down.
    pub fn is_press(self) -> bool {
        matches!(self, How::Click(0 | 1) | How::Key { press: true })
    }
}

/// What a key did to an open menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Key<A> {
    /// The selection moved (or stayed: nothing else can act).
    Moved,
    Pick(A),
    Close,
    /// Not a menu key: the menu closes, the key goes on.
    Other,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Menu<A> {
    pub items: Vec<Item<A>>,
    /// The selected item (keyboard); a pointer-opened menu starts with none.
    pub sel: Option<usize>,
    pub at: At,
}

impl<A: Copy> Menu<A> {
    /// From the keyboard the first item that can act is selected.
    pub fn new(items: Vec<Item<A>>, at: At, keyboard: bool) -> Self {
        let sel = if keyboard { items.iter().position(Item::enabled) } else { None };
        Menu { items, sel, at }
    }

    /// The next item that can act from the selection, `down` or up,
    /// wrapping; none can: the selection stays.
    fn step(&mut self, down: bool) {
        let n = self.items.len();
        if n == 0 {
            return;
        }
        let start = self.sel.unwrap_or(if down { n - 1 } else { 0 });
        for k in 1..=n {
            let i = if down { (start + k) % n } else { (start + n - k) % n };
            if self.items[i].enabled() {
                self.sel = Some(i);
                return;
            }
        }
    }

    /// A key (as GPUI names it; `shift` held; `press`: a new press, not a
    /// held key's repeat) while the menu is open. A repeat of Enter or
    /// Space picks nothing.
    pub fn key(&mut self, key: &str, shift: bool, press: bool) -> Key<A> {
        match key {
            "tab" => self.step(!shift),
            "down" => self.step(true),
            "up" => self.step(false),
            "home" => {
                self.sel = None;
                self.step(true);
            }
            "end" => {
                self.sel = None;
                self.step(false);
            }
            "enter" | "space" => {
                return match self.sel.and_then(|i| self.items.get(i)).filter(|it| it.enabled() && press) {
                    Some(it) => Key::Pick(it.act),
                    None => Key::Moved,
                };
            }
            "escape" => return Key::Close,
            "shift" | "control" | "alt" | "platform" | "function" => {}
            _ => return Key::Other,
        }
        Key::Moved
    }
}

/// The menu on the overlay box (design.md §8.8); `pick` acts on an item
/// (a click, with its count: [`How`]; or Enter through [`Menu::key`]),
/// `close` takes a mouse-down outside it.
pub(crate) fn render<A: Copy + 'static>(
    id: &'static str,
    menu: &Menu<A>,
    p: &Palette,
    pick: fn(&mut AppView, A, How, &mut Window, &mut Context<AppView>),
    close: fn(&mut AppView, &MouseDownEvent),
    cx: &Context<AppView>,
) -> Stateful<Div> {
    let mut col = crate::overlay::frame(v_flex(), p)
        .id(id)
        .font_family(crate::fonts::SANS)
        .min_w(px(236.))
        .max_w(px(360.))
        .py(px(4.))
        .occlude()
        .on_mouse_down_out(cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
            close(this, ev);
            cx.notify();
        }));
    for (i, it) in menu.items.iter().enumerate() {
        let on = it.enabled();
        let selected = menu.sel == Some(i);
        let act = it.act;
        // The keyboard's row: the hover fill and a 1 px focus frame (§5).
        let mut row = h_flex()
            .id(("menu-item", i))
            .relative()
            .h(px(32.))
            .px(px(16.))
            .gap(px(24.))
            .items_center()
            .whitespace_nowrap()
            .when(selected, |d| d.bg(p.card_hi).child(div().absolute().inset_0().border_1().border_color(p.focus_k)))
            .child(
                h_flex()
                    .flex_1()
                    .gap(px(8.))
                    .text_size(px(size::UI))
                    .text_color(if on { p.ink } else { p.dim_k })
                    .child(it.label.clone())
                    .when_some(it.confirm.clone(), |d, c| d.child(crate::app::display(c, size::D_S, p.err_k))),
            )
            .when_some(it.key, |d, k| d.child(mono(k, size::META, p.dim_k)));
        if on {
            row = row.cursor_pointer().hover(|d| d.bg(p.card_hi)).on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                cx.stop_propagation();
                pick(this, act, How::Click(ev.click_count()), window, cx);
            }));
        } else if let Some(why) = it.why_not.clone() {
            row = row.tooltip(move |window, cx| crate::overlay::tip(why.clone(), window, cx));
        }
        col = col.child(row);
    }
    col
}

#[cfg(test)]
mod tests {
    use super::{At, How, Item, Key, Menu};

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum A {
        One,
        Two,
        Three,
    }

    fn menu(keyboard: bool) -> Menu<A> {
        let items = vec![Item::new(A::One, "one").off("no"), Item::new(A::Two, "two"), Item::new(A::Three, "three")];
        Menu::new(items, At::Row(1), keyboard)
    }

    #[test]
    fn the_keys_walk_the_items_that_can_act() {
        // From the keyboard the first one that can act is selected.
        let k = |m: &mut Menu<A>, key: &str| m.key(key, false, true);
        let mut m = menu(true);
        assert_eq!(m.sel, Some(1));
        assert_eq!(k(&mut m, "down"), Key::Moved);
        assert_eq!(m.sel, Some(2));
        // Wrapping, past the one that cannot act.
        assert_eq!(k(&mut m, "down"), Key::Moved);
        assert_eq!(m.sel, Some(1));
        assert_eq!(k(&mut m, "up"), Key::Moved);
        assert_eq!(m.sel, Some(2));
        assert_eq!(k(&mut m, "home"), Key::Moved);
        assert_eq!(m.sel, Some(1));
        assert_eq!(k(&mut m, "end"), Key::Moved);
        assert_eq!(m.sel, Some(2));
        // Tab goes down, Shift+Tab up.
        assert_eq!((k(&mut m, "tab"), m.sel), (Key::Moved, Some(1)));
        assert_eq!((m.key("tab", true, true), m.sel), (Key::Moved, Some(2)));
        assert_eq!(k(&mut m, "enter"), Key::Pick(A::Three));
        assert_eq!(k(&mut m, "space"), Key::Pick(A::Three));
        assert_eq!(k(&mut m, "shift"), Key::Moved, "a modifier alone changes nothing");
        assert_eq!(k(&mut m, "escape"), Key::Close);
        assert_eq!(k(&mut m, "x"), Key::Other);
        // From the pointer nothing is selected: Enter picks nothing.
        let mut m = menu(false);
        assert_eq!((m.sel, k(&mut m, "enter")), (None, Key::Moved));
        assert_eq!(k(&mut m, "up"), Key::Moved);
        assert_eq!(m.sel, Some(2));
        // Nothing can act: the selection stays.
        let mut m = Menu::new(vec![Item::new(A::One, "one").off("no")], At::Row(1), true);
        assert_eq!((m.sel, k(&mut m, "down"), m.sel), (None, Key::Moved, None));
    }

    /// A held Enter or Space picks once, on its press: its repeats pick
    /// nothing (they still move nothing and close nothing). A click picks
    /// on its first click only.
    #[test]
    fn a_held_key_picks_once() {
        let mut m = menu(true);
        assert_eq!(m.key("enter", false, true), Key::Pick(A::Two));
        for _ in 0..5 {
            assert_eq!(m.key("enter", false, false), Key::Moved, "a repeat");
            assert_eq!(m.key("space", false, false), Key::Moved, "a repeat");
        }
        assert_eq!(m.sel, Some(1), "where it was");
        assert_eq!(m.key("enter", false, true), Key::Pick(A::Two), "pressed again");
        assert!(How::Click(1).is_press() && How::Key { press: true }.is_press());
        assert!(!How::Click(2).is_press() && !How::Click(3).is_press() && !How::Key { press: false }.is_press());
    }
}
