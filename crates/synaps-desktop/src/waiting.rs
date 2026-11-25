//! What waits on the user, in Ctrl+J's order (synaps-cards
//! `docs/design.md` v1.2, §5.1 and §6.1; the D2b row of §9.3): authorize
//! prompts first, then the cards the agent waits on, then the open ones.
//! Within each kind, the focused pane's before the other visible panes'
//! (in reading order), and the oldest first within a pane.
//!
//! A pure function of what the window shows, so the keys that reach
//! what waits share one order:
//! - **Ctrl+J** (D2b, [`crate::ask_keys`]): the first of this order, over
//!   this window's visible panes, and a batch of its pane's cards;
//! - **Ctrl+Shift+J and Ctrl+P's Waiting group** (D3): the same order,
//!   every open session at one [`Waiting::place`], so the oldest comes
//!   first whatever pane shows it.

/// What kind of thing waits: the order's first key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Waits {
    /// A permission prompt: a card never resolves one (§5.1), and Ctrl+J
    /// visits these first.
    Prompt,
    /// A card the agent waits on (`wait: true`, NEEDS YOU).
    Wait,
    /// An open card the agent asked without waiting.
    Open,
}

/// One thing that waits, where it is, and how old it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Waiting<K> {
    pub(crate) waits: Waits,
    /// Where it is: 0 for the focused pane, then each other visible pane
    /// in reading order. One place for everything (D3) orders by kind and
    /// age alone.
    pub(crate) place: usize,
    /// How old it is within its place: lower is older (a transcript row's
    /// index; D3: the time it was raised).
    pub(crate) age: u64,
    /// What it is, for the caller.
    pub(crate) key: K,
}

/// `items` in Ctrl+J's order: by kind, then place, then age. Stable, so
/// two of one age keep the order they came in.
pub(crate) fn order<K>(mut items: Vec<Waiting<K>>) -> Vec<Waiting<K>> {
    items.sort_by_key(|w| (w.waits, w.place, w.age));
    items
}

/// The batch a Ctrl+J entry freezes (§6.1, F13): the cards (no prompt) at
/// `place`, in the order's order. Cards that come later are not in it.
pub(crate) fn batch<K>(items: Vec<Waiting<K>>, place: usize) -> Vec<K> {
    order(items).into_iter().filter(|w| w.place == place && w.waits != Waits::Prompt).map(|w| w.key).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(waits: Waits, place: usize, age: u64, key: &'static str) -> Waiting<&'static str> {
        Waiting { waits, place, age, key }
    }

    fn keys(v: Vec<Waiting<&'static str>>) -> Vec<&'static str> {
        order(v).into_iter().map(|w| w.key).collect()
    }

    /// §6.1: prompts, then wait cards, then open cards, wherever each is:
    /// another pane's prompt comes before this pane's wait card, and
    /// another pane's wait card before this pane's open card.
    #[test]
    fn prompts_then_wait_cards_then_open_cards() {
        let v = vec![
            w(Waits::Open, 0, 1, "open here"),
            w(Waits::Wait, 1, 5, "wait there"),
            w(Waits::Prompt, 2, 9, "prompt there"),
            w(Waits::Wait, 0, 7, "wait here"),
            w(Waits::Prompt, 0, 3, "prompt here"),
        ];
        assert_eq!(keys(v), ["prompt here", "prompt there", "wait here", "wait there", "open here"]);
    }

    /// Within a kind, this pane first, then the others in reading order;
    /// within a pane, the oldest first.
    #[test]
    fn this_pane_first_then_reading_order_then_the_oldest() {
        let v = vec![w(Waits::Open, 2, 1, "c"), w(Waits::Open, 0, 8, "a2"), w(Waits::Open, 1, 0, "b"), w(Waits::Open, 0, 3, "a1")];
        assert_eq!(keys(v), ["a1", "a2", "b", "c"]);
    }

    /// One place for all (D3's Ctrl+Shift+J): the oldest of a kind first,
    /// whichever session it is in.
    #[test]
    fn one_place_orders_by_age_alone() {
        let v = vec![w(Waits::Open, 0, 2, "open old"), w(Waits::Wait, 0, 9, "wait new"), w(Waits::Wait, 0, 4, "wait old")];
        assert_eq!(keys(v), ["wait old", "wait new", "open old"]);
    }

    /// The batch: its pane's cards in the order, no prompt, no other
    /// pane's card.
    #[test]
    fn a_batch_is_its_panes_cards_in_order() {
        let v = vec![
            w(Waits::Open, 1, 1, "open"),
            w(Waits::Prompt, 1, 0, "prompt"),
            w(Waits::Wait, 0, 2, "elsewhere"),
            w(Waits::Wait, 1, 6, "wait"),
            w(Waits::Open, 1, 0, "older open"),
        ];
        assert_eq!(batch(v, 1), ["wait", "older open", "open"]);
    }

    /// Two of one age keep the order they came in.
    #[test]
    fn ties_keep_their_order() {
        let v = vec![w(Waits::Wait, 0, 1, "first"), w(Waits::Wait, 0, 1, "second")];
        assert_eq!(keys(v), ["first", "second"]);
    }
}
