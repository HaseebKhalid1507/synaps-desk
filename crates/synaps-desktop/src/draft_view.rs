//! Take, edit, hold (synaps-cards `docs/blocks.md` v1.3 §3.2, §3.7's who
//! may edit, §3.8, §6.1, §6.2; §8.2's D8 row, its first half: D8a). Where
//! [`crate::drafts`]' drafts are drawn, and how keys and clicks reach them.
//!
//! - **Taking.** `e` in the block walk, the header's `edit` (its hover
//!   action), or a click on an editable part (a step's label or its
//!   strike gutter, its stop gutter, a note's text or title) takes the
//!   block: a draft over its **bound** incarnation at its revision `B`,
//!   then the part's action. A take acts on the painted target (§6.2): the
//!   block's row and incarnation as painted, on the bound board now, a
//!   click through the lease's down and up and the just-moved rule as a
//!   fold's is; another incarnation says `this block changed · look again`.
//!   Status, metric and file blocks aren't editable; a pending block (no
//!   bound incarnation) isn't either.
//! - **Who may.** Only with the session's input authority (§3.1:
//!   `Live::input_gate`, as a card's answer): without it the block takes
//!   nothing and says why, `edit from client #3 · /takeover` in an
//!   exclusive session another client owns, else the gate's reason.
//! - **Held.** The block draws the draft over `B` ([`crate::drafts`]),
//!   headed `EDITING · r4` in the focus colour with its spine and ring
//!   (an entered card's), `· 2 agent changes` from the agent's first
//!   change since `B`, and `the agent keeps working` (a hold doesn't pause
//!   it). Its footer lists what the pre-check finds would fail now, and
//!   says why nothing can be handed back, if nothing can. The pane holds
//!   the keys on its own focus ([`EditMode::focus`], never the window's,
//!   whose keys answer a prompt).
//! - **Keys** (fresh, bare presses only, from the first frame that paints
//!   the ring; Shift+J and Shift+K a new press with Shift alone):
//!   - plan: `j`/`k` (↓/↑) the step cursor; `Shift+J`/`Shift+K` move the
//!     step; `x` strikes or unstrikes it; `b` toggles a stop before it;
//!     `r` renames it (an inline field); `a` adds a step after it (a `u-`
//!     id, its label in a field). A step that ran can be renamed and
//!     moved, never struck nor stopped: `already ran`.
//!   - note: `r` its text, `t` its title, each in a field.
//!   - diff: `j`/`k` the hunk cursor; `y` keeps it, `n` drops it, while the
//!     diff is a proposal.
//!   - Enter hands back: D8b sends it (B4's memo, D2a's send). Here it
//!     sends nothing, and says so.
//!   - Esc: with no changes, the draft goes and the keys go back to the
//!     composer; with changes, the first Esc says `Esc again discards ·
//!     Enter hands back`, the second discards. There is no dialog. Never
//!     the window's: a second Esc can't deny a prompt.
//!   - Any other printable key goes to the composer, typed; the draft
//!     stays held.
//! - **Fields** are the kit's inputs: an IME composition owns every key in
//!   them (§6.2: text fields and IME are exempt from the just-moved rule).
//!   Their text is the draft's as it is typed. Enter in a one-line field,
//!   Tab, and Esc leave the field to the block's keys, each only on a new,
//!   bare press that met no composition (an entered card's [`FieldKey`]
//!   rule).
//! - **Stale and orphaned.** A draft whose incarnation ended draws in its
//!   row with the header's `the agent rewrote this block (r1)` or `this
//!   block is gone`, and one with no row here (gone from the transcript,
//!   or compacted: `compacted · this draft has no block`) in the drafts
//!   dock over the composer. Either offers Copy to composer (line 1's
//!   words) and Discard, by click, `c` and `Esc` twice.
//!
//! [`FieldKey`]: crate::ask_keys::FieldKey

// The tests-first commit's shape of the module: its words and types, its
// logic not yet written (every function a stand-in).
#![allow(dead_code, unused_imports, unused_variables)]

use gpui_kit::component::input::{InputState, TextareaState};
use gpui_kit::*;
use synaps_client::blocks::Tag;

use crate::app::AppView;

/// The key context of a pane's draft keys.
pub(crate) const EDIT_CTX: &str = "BlockEdit";

/// A draft field's inputs: Tab has no binding there (`NoAction`), and
/// comes as a key.
pub(crate) const EDIT_INPUT_CTX: &str = "BlockEdit > Input";

/// Enter on a held block, in D8a: nothing is sent.
pub(crate) const NOT_SENT: &str = "handing back comes with D8b · nothing was sent";

/// The first Esc with changes (§3.2).
pub(crate) const ESC_AGAIN: &str = "Esc again discards · Enter hands back";

/// A hold doesn't pause the agent (§3.2).
pub(crate) const KEEPS_WORKING: &str = "the agent keeps working";

/// Not a kind the user edits.
pub(crate) const NOT_EDITABLE: &str = "this block isn't editable";

/// An editable part of a block: the action a click on it takes (an item
/// by its id's hash, as painted).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Part {
    /// Take it, nothing more (the header's `edit`).
    Take,
    Label(u64),
    Strike(u64),
    Stop(u64),
    Keep(u64),
    Drop(u64),
    Text,
    Title,
    Copy,
    Discard,
    HandBack,
}

/// A pane's draft keys (module docs).
#[derive(Default)]
pub(crate) struct EditMode {
    pub focus: Option<FocusHandle>,
    pub note: Option<(u64, String)>,
}

impl EditMode {
    pub(crate) fn holds(&self, window: &Window) -> bool {
        false
    }

    pub(crate) fn inputs(&self) -> (Option<&Entity<InputState>>, Option<&Entity<TextareaState>>) {
        (None, None)
    }
}

/// The hash a painted item carries.
pub(crate) fn item_hash(id: &cards_spec::block::ItemId) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    id.as_str().hash(&mut h);
    h.finish()
}

impl AppView {
    pub(crate) fn take_block(&mut self, row: u64, tag: Tag, part: Part, window: &mut Window, cx: &mut Context<Self>) -> bool {
        false
    }
}
