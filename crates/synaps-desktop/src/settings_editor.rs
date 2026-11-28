//! The edit guard on Settings' kit inputs (settings-overhaul PLAN §3.6
//! Part 5). Each input's text is capped (the kit's validator refuses an
//! edit past it before it is recorded), and its undo history is charged
//! from the kit's own edit events and kept within a byte budget: past the
//! budget the input is rebuilt from its text, its history dropped with the
//! old entity. The kit offers no other bound but a depth of 1,000 steps.

use gpui_kit::component::input::{Input, InputEditsEvent, InputEvent, InputState};
use gpui_kit::{AppContext as _, Context, Entity, Focusable as _, ParentElement as _, Styled as _, Subscription, Window};

use crate::app::AppView;

/// The two short fields' caps and budgets (PLAN §3.6 Part 5's table).
pub(crate) const VALUE_CAP: usize = 4096;
pub(crate) const VALUE_BUDGET: u32 = 65_536;
pub(crate) const FIND_CAP: usize = 256;
pub(crate) const FIND_BUDGET: u32 = 16_384;

/// Under an input whose history was rebuilt away, once.
pub(crate) const CLEARED: &str = "Undo history cleared to keep memory in bounds.";

/// An edit that replaced something, or one of several in its batch, may
/// add a `Change` and its cursor records to a step.
const STEP: u32 = 512;

/// One input's cap, budget and charge. No heap of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EditGuard {
    cap: u32,
    budget: u32,
    charge: u32,
    /// Its history was cleared to keep within the budget: the note shows.
    pub(crate) cleared: bool,
    /// A paste would have passed the cap and was refused: the note shows.
    pub(crate) refused: bool,
}

const _: () = assert!(std::mem::size_of::<EditGuard>() <= 64);

/// What a paste does ([`EditGuard::paste`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pasted {
    /// The kit pastes it.
    Go,
    /// The text would pass the cap: nothing applies.
    Refuse,
    /// Its charge would pass the budget: the input is rebuilt first, so
    /// the paste itself stays undoable.
    RebuildFirst,
}

impl EditGuard {
    pub(crate) const fn new(cap: usize, budget: u32) -> EditGuard {
        EditGuard { cap: cap as u32, budget, charge: 0, cleared: false, refused: false }
    }

    pub(crate) fn value() -> EditGuard {
        EditGuard::new(VALUE_CAP, VALUE_BUDGET)
    }

    pub(crate) fn find() -> EditGuard {
        EditGuard::new(FIND_CAP, FIND_BUDGET)
    }

    pub(crate) fn cap(&self) -> usize {
        self.cap as usize
    }

    #[cfg(test)]
    pub(crate) fn charge(&self) -> u32 {
        self.charge
    }

    /// One edit's charge: twice what it replaced and inserted (the kit
    /// keeps both, and either buffer can be twice its length), and
    /// [`STEP`] when it replaced something or was one of `several`.
    pub(crate) fn charge_of(replaced: usize, inserted: usize, several: bool) -> u32 {
        let text = 2 * (replaced + inserted);
        (text as u32).saturating_add(if replaced > 0 || several { STEP } else { 0 })
    }

    /// An edit event's charge added: whether the budget is passed (the
    /// input is to be rebuilt). An undo or a redo is charged too: the
    /// event doesn't mark them, so the charge only ever overcounts.
    pub(crate) fn edits(&mut self, ev: &InputEditsEvent) -> bool {
        let several = ev.edits().len() > 1;
        for (e, _) in ev.edits() {
            let replaced = e.old_end_byte.saturating_sub(e.start_byte);
            let inserted = e.new_end_byte.saturating_sub(e.start_byte);
            self.charge = self.charge.saturating_add(Self::charge_of(replaced, inserted, several));
        }
        self.charge > self.budget
    }

    /// A paste of `text` over `selected` bytes of `len`: within the cap,
    /// and within the budget.
    pub(crate) fn paste(&self, len: usize, selected: usize, text: usize) -> Pasted {
        if len - selected.min(len) + text > self.cap() {
            Pasted::Refuse
        } else if self.charge.saturating_add(Self::charge_of(selected, text, false)) > self.budget {
            Pasted::RebuildFirst
        } else {
            Pasted::Go
        }
    }

    /// The input was rebuilt from its text: nothing charged.
    pub(crate) fn rebuilt(&mut self) {
        self.charge = 0;
        self.cleared = true;
    }

    /// A fresh input (opened, or retired empty).
    pub(crate) fn fresh(&mut self) {
        *self = EditGuard::new(self.cap(), self.budget);
    }
}

/// Which of Settings' two short fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Field {
    /// The shared value field (`settings_input`).
    Value,
    /// The find box (`settings_find`).
    Find,
}

/// The fields' inputs just replaced, drawn once more, hidden and unfocused,
/// then dropped with the frame. The kit's window state keeps the input
/// that last drew focused until that input draws unfocused
/// (`gpui-component 0.9.1 src/input/state.rs:371–394`,
/// `sync_focused_input_registry`). A field closed with its focus (Enter,
/// Esc, the tab's close) never draws again, so without this frame its
/// input, text and history would stay held.
pub(crate) fn render_retired(retired: &mut [Option<Entity<InputState>>; 2]) -> Vec<gpui_kit::Div> {
    retired
        .iter_mut()
        .filter_map(Option::take)
        .map(|old| gpui_kit::div().absolute().size_0().overflow_hidden().child(Input::new(&old).appearance(false)))
        .collect()
}

impl AppView {
    pub(crate) fn field_input(&self, f: Field) -> &Entity<InputState> {
        match f {
            Field::Value => &self.settings_input,
            Field::Find => &self.settings_find,
        }
    }

    pub(crate) fn field_guard(&mut self, f: Field) -> &mut EditGuard {
        match f {
            Field::Value => &mut self.settings_input_guard,
            Field::Find => &mut self.settings_find_guard,
        }
    }

    /// A new entity for field `f` holding `text` (capped by the kit's
    /// validator), and its subscriptions; the old one goes, with its
    /// history and its buffers. With `keep`, the old one's caret,
    /// selection, scroll and focus carry over (a rebuild).
    pub(crate) fn new_field(&mut self, f: Field, text: &str, keep: bool, window: &mut Window, cx: &mut Context<Self>) {
        let cap = self.field_guard(f).cap();
        let old = self.field_input(f).clone();
        let (sel, reversed, scroll, focused) = {
            let o = old.read(cx);
            (o.selected_range(), o.is_selection_reversed(), o.scroll_offset(), o.focus_handle(cx).is_focused(window))
        };
        let placeholder = (f == Field::Find).then_some("find a setting");
        let input = cx.new(|cx| {
            let mut s = InputState::new(window, cx).validate(move |t, _| t.len() <= cap);
            if let Some(p) = placeholder {
                s = s.placeholder(p);
            }
            s.set_value(text.to_string(), window, cx);
            s
        });
        if keep {
            input.update(cx, |s, cx| {
                s.set_selected_range_with_direction(sel, reversed, cx);
                s.set_scroll_offset(scroll, cx);
            });
        }
        let subs = Self::field_subs(f, &input, window, cx);
        match f {
            Field::Value => (self.settings_input, self.settings_input_subs) = (input.clone(), subs),
            Field::Find => (self.settings_find, self.settings_find_subs) = (input.clone(), subs),
        }
        match (keep, focused) {
            (true, true) => input.update(cx, |s, cx| s.focus(window, cx)),
            // Not left on an input that is gone.
            (false, true) => window.focus(&self.focus, cx),
            _ => {}
        }
        // The old input goes through one more frame (see `render_retired`).
        // Of two replaced before a frame, only the first was ever drawn, so
        // only it can be held: the second is dropped now.
        let slot = &mut self.settings_retired[f as usize];
        if slot.is_none() {
            *slot = Some(old);
        }
        cx.notify();
    }

    /// Field `f`'s subscriptions: its events, and its edits charged.
    pub(crate) fn field_subs(f: Field, input: &Entity<InputState>, window: &mut Window, cx: &mut Context<Self>) -> [Subscription; 2] {
        let events = match f {
            Field::Find => cx.subscribe_in(input, window, |this: &mut Self, input, ev: &InputEvent, _window, cx| {
                if let InputEvent::Change = ev {
                    let text = input.read(cx).value().to_string();
                    this.settings_find(text, cx);
                }
            }),
            Field::Value => cx.subscribe_in(input, window, |this: &mut Self, _input, ev: &InputEvent, window, cx| match ev {
                InputEvent::PressEnter { .. } if !this.enter_is_the_picks() => {
                    this.settings_commit(cx);
                    window.focus(&this.focus, cx);
                    this.retire_value_field(window, cx);
                }
                // Clicked away: the edit is left as it was.
                InputEvent::Blur => {
                    let was = this.settings.editing.take();
                    if was.is_some() {
                        this.views_changed(cx);
                    }
                    this.retire_value_field(window, cx);
                }
                _ => {}
            }),
        };
        let edits = cx.subscribe_in(input, window, move |this: &mut Self, _input, ev: &InputEditsEvent, window, cx| {
            if this.field_guard(f).edits(ev) {
                // Never inside the input's own handler.
                cx.defer_in(window, move |this, window, cx| this.rebuild_field(f, window, cx));
            }
        });
        [events, edits]
    }

    /// Field `f` rebuilt from its own text, at its budget.
    pub(crate) fn rebuild_field(&mut self, f: Field, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.field_input(f).read(cx).value().to_string();
        self.new_field(f, &text, true, window, cx);
        self.field_guard(f).rebuilt();
        self.views_changed(cx);
    }

    /// The value field closed (a commit, Esc, a blur, the tab's close): an
    /// empty one in its place, deferred out of its own handlers.
    pub(crate) fn retire_value_field(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.defer_in(window, |this, window, cx| {
            if this.settings.editing.is_none() {
                this.new_field(Field::Value, "", false, window, cx);
                this.settings_input_guard.fresh();
            }
        });
    }

    /// The Settings tab is gone: both fields replaced by empty ones (their
    /// text, history and buffers go with the old entities), unless they
    /// are fresh already.
    pub(crate) fn settings_tab_closed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.model.tabs.iter().any(|t| t.kind == crate::tabs::Kind::Settings) {
            return;
        }
        for f in [Field::Value, Field::Find] {
            let g = *self.field_guard(f);
            let used = g != EditGuard::new(g.cap(), self.field_budget(f)) || !self.field_input(f).read(cx).value().is_empty();
            if used {
                self.settings.editing = None;
                self.new_field(f, "", false, window, cx);
                self.field_guard(f).fresh();
            }
        }
        self.settings_find(String::new(), cx);
    }

    fn field_budget(&self, f: Field) -> u32 {
        match f {
            Field::Value => VALUE_BUDGET,
            Field::Find => FIND_BUDGET,
        }
    }

    /// A paste into field `f`, before the kit's (capture phase): refused
    /// whole past the cap; past the budget, the input rebuilt first and the
    /// paste then made on the new one.
    pub(crate) fn field_paste(&mut self, f: Field, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) else { return };
        // A single-line input drops the newlines.
        let text: String = text.chars().filter(|c| !matches!(c, '\n' | '\r')).collect();
        let (len, sel) = {
            let s = self.field_input(f).read(cx);
            (s.value().len(), s.selected_range().len())
        };
        match self.field_guard(f).paste(len, sel, text.len()) {
            Pasted::Go => {}
            Pasted::Refuse => {
                self.field_guard(f).refused = true;
                cx.stop_propagation();
                self.views_changed(cx);
            }
            Pasted::RebuildFirst => {
                // The new input isn't drawn yet, so no action reaches it:
                // the paste is made on it as the kit makes one, one atomic
                // edit over the selection the rebuild kept.
                cx.stop_propagation();
                cx.defer_in(window, move |this, window, cx| {
                    this.rebuild_field(f, window, cx);
                    this.field_input(f).clone().update(cx, |s, cx| s.replace(text, window, cx));
                });
            }
        }
    }

    /// The line under field `f`: a paste refused, or its history cleared.
    pub(crate) fn field_note(&self, f: Field) -> Option<String> {
        let g = match f {
            Field::Value => &self.settings_input_guard,
            Field::Find => &self.settings_find_guard,
        };
        if g.refused {
            Some(format!("Not pasted: this field takes at most {} bytes.", crate::usage::thousands(g.cap() as u64)))
        } else {
            g.cleared.then(|| CLEARED.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The charge of each kind of edit, and each budget at its cap.
    #[test]
    fn an_edits_charge_is_its_text_twice_and_its_step() {
        assert_eq!(EditGuard::charge_of(0, 1, false), 2, "a lone insertion: no step (a typing run's)");
        assert_eq!(EditGuard::charge_of(4096, 4096, false), 16_896, "a replacement at the value field's cap");
        assert_eq!(EditGuard::charge_of(256, 256, false), 1_536, "and at the find field's");
        assert_eq!(EditGuard::charge_of(10, 0, false), 532, "a deletion");
        assert_eq!(2 * EditGuard::charge_of(0, 1, true), 1_028, "a batch of two insertions");
        for (mut g, at_cap, fits) in [(EditGuard::value(), 16_896, 3), (EditGuard::find(), 1_536, 10)] {
            let budget = if g.cap() == VALUE_CAP { VALUE_BUDGET } else { FIND_BUDGET };
            assert!(fits * at_cap <= budget, "{fits} replacements at the cap fit");
            assert!((fits + 1) * at_cap > budget, "one more passes");
            let full = g.cap();
            assert_eq!(g.paste(full, full, full), Pasted::Go, "a whole replacement");
            assert_eq!(g.paste(full, 0, 1), Pasted::Refuse, "one byte past the cap");
            assert_eq!(g.paste(full - 1, 0, 1), Pasted::Go, "at the cap");
            for _ in 0..fits {
                g.charge += at_cap;
            }
            assert_eq!(g.paste(full, full, full), Pasted::RebuildFirst, "past the budget: rebuilt first");
            g.rebuilt();
            assert_eq!((g.charge(), g.cleared), (0, true));
            g.fresh();
            assert_eq!((g.charge(), g.cleared, g.refused), (0, false, false));
        }
    }
}
