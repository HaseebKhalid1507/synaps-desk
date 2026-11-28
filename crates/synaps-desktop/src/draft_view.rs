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
//! - **A field holds what the draft holds** (§1.8). A one-line field (a
//!   label, a title) takes no text past its cap, typed, pasted or marked
//!   by an IME: its characters (B1's) and the draft's bytes; the block
//!   says why (`at most 100 characters`, or the draft's cap). The text
//!   area (a note's text) and every field after the draft took a change
//!   hold the draft's value, cut or refused as the draft took it, in the
//!   same event, the caret kept. A field's copy is one: a pane has one
//!   field, and one draft is edited in one field on the desk (opening it
//!   in another pane closes the first, its text the draft's). The kit's
//!   undo history is cleared once the changes it may hold pass
//!   [`FIELD_HISTORY_MAX`] (each counted as the kit keeps it, the text it
//!   took out and the text it put in, [`kept_of`]), the text and caret
//!   kept. A field holds at most its draft's value, so at most
//!   [`crate::drafts::DRAFT_MAX_BYTES`], and its history twice that,
//!   plus one change.
//! - **Stale and orphaned.** A draft whose incarnation ended draws in its
//!   row with the header's `the agent rewrote this block (r1)` or `this
//!   block is gone`, and one with no row here (gone from the transcript,
//!   or compacted: `compacted · this draft has no block`, or doubled by a
//!   follow) in the drafts dock over the composer. Either offers Copy to
//!   composer (line 1's words) and Discard, by click, `c` and `Esc` twice.
//! - **Held in place** (§3.4). The board the desk draws keeps each block a
//!   draft is drawn in where it was taken ([`crate::drafts::hold_places`],
//!   [`AppView::drafts_in_place`]): the agent's `move` waits, and so does
//!   its `collapse` (a draft draws whole). Let go, the next board draws it
//!   where the agent put it, once.
//! - **A session that follows to a new id** takes its drafts with it, and
//!   every pane's editing of them ([`AppView::drafts_follow`]).
//! - **The footer's height** is the pre-check's: when what it says changes
//!   (the bound board arrived, or another one), the row is measured again
//!   in every pane.
//!
//! [`FieldKey`]: crate::ask_keys::FieldKey

use std::cell::Cell;
use std::hash::{BuildHasher as _, Hasher as _};
use std::rc::Rc;

use cards_spec::block::{ItemId, Ref, StepState};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use synaps_client::blocks::{Body, Shown, Tag};

use crate::app::AppView;
use crate::drafts::{Draft, Fate};
use crate::fonts::{self, TextPx as _};
use crate::theme::{Metrics, Palette};

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

/// The changes a field's undo history may hold (bytes: each as the kit
/// keeps it, [`kept_of`]) before it is cleared ([`Field::history`]).
pub(crate) const FIELD_HISTORY_MAX: usize = 2 * crate::drafts::DRAFT_MAX_BYTES;

/// A field's text past its characters' cap.
pub(crate) fn at_most(n: usize) -> String {
    format!("at most {n} characters")
}

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

/// What a draft field edits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FieldOf {
    Label(ItemId),
    Title,
    Text,
}

impl FieldOf {
    /// Its text's cap in characters (B1's).
    pub(crate) fn chars(&self) -> usize {
        match self {
            FieldOf::Label(_) => cards_spec::block::STEP_LABEL_MAX,
            FieldOf::Title => cards_spec::block::TITLE_MAX,
            FieldOf::Text => cards_spec::block::NOTE_TEXT_MAX,
        }
    }
}

/// An open draft field: the kit's input, one line or a text area.
pub(crate) struct Field {
    pub of: FieldOf,
    pub line: Option<Entity<InputState>>,
    pub long: Option<Entity<TextareaState>>,
    /// The bytes its text may have: the draft's value's and the draft's
    /// room ([`AppView::field_room`]), what a one-line field takes.
    pub room: Rc<Cell<usize>>,
    /// What the one-line field refused, as it came (1: past its
    /// characters, 2: past the draft's room), said at the next frame.
    pub refused: Rc<Cell<u8>>,
    /// What the kit's undo history may hold since it was last cleared
    /// (bytes, [`FIELD_HISTORY_MAX`]): each change as the kit keeps it
    /// ([`kept_of`], [`Field::saw`]), a composition's too.
    pub history: usize,
    /// Its text as last seen (bytes, and a hash of it and its marked
    /// range).
    seen: (usize, u64),
    _sub: Subscription,
    _watch: Subscription,
}

/// What the kit's undo history keeps of one change to a field's text
/// (bytes): the text the change took out and the text it put in
/// (gpui-base 0.9.1 `push_history`, state.rs:3359–3404, into
/// `UndoManager::record_transaction`, undo_manager.rs:100–118; a
/// composition's changes kept together until it ends, 144–162). The text
/// was `before` bytes and is `after`; `marked`: `Some` bytes of it marked
/// now, a preedit (`replace_and_mark_text_in_range`, 4892–5033), which
/// took out exactly what its marked text replaced. Any other change takes
/// out at most the text before it and puts in at most the text after: a
/// composition's commit (`replace_text_in_range`, 4636–4889) takes out
/// its marked text, not the text the composition began on, and puts in
/// the candidate, whatever their lengths.
pub(crate) fn kept_of(before: usize, after: usize, marked: Option<usize>) -> usize {
    match marked {
        Some(m) => (before + m).saturating_sub(after) + m,
        None => before + after,
    }
}

impl Field {
    /// Its text seen now, `v`, `marked` its marked range (UTF-16, the
    /// kit's): the change from the text last seen counted in
    /// [`Self::history`] as the kit keeps it ([`kept_of`]), once, by
    /// whichever of its `Change` and its notify sees it first (each comes
    /// for a change; a preedit, and a composition cancelled by an empty
    /// one, come with a notify only). The same text and marked range seen
    /// again counts nothing (a notify with nothing changed). Whether it
    /// was new.
    fn saw(&mut self, v: &str, marked: &Option<std::ops::Range<usize>>) -> bool {
        let look = seen_hash(v, marked);
        if self.seen.1 == look {
            return false;
        }
        let mark = marked.as_ref().map(|r| utf16_bytes(v, r));
        self.history += kept_of(self.seen.0, v.len(), mark);
        self.seen = (v.len(), look);
        true
    }

    /// Its text set to `value`, its undo history cleared and no
    /// composition left (the kit's `set_value`, state.rs:1281–1303):
    /// nothing counted.
    fn cleared(&mut self, value: &str) {
        self.history = 0;
        self.seen = (value.len(), seen_hash(value, &None));
    }
}

/// What the field says of a composition refused because its history
/// would pass [`FIELD_HISTORY_MAX`].
pub(crate) const COMPOSED_LONG: &str = "this composition is too long to keep · type it in parts";

/// A note's whole text and title, at their characters' most bytes, fit
/// in a draft's room (§1.8): a text area's characters bound what it is
/// marked with.
const _: () = assert!(
    std::mem::size_of::<Draft>() + 4 * (cards_spec::block::NOTE_TEXT_MAX + cards_spec::block::TITLE_MAX) <= crate::drafts::DRAFT_MAX_BYTES
);

/// The hash of text `v` and its marked range (`Field::seen`).
fn seen_hash(v: &str, marked: &Option<std::ops::Range<usize>>) -> u64 {
    let mut h = std::hash::DefaultHasher::new();
    std::hash::Hash::hash(&(v, marked), &mut h);
    h.finish()
}

/// The bytes of text `v`'s UTF-16 range `r` (the kit's marked range).
fn utf16_bytes(v: &str, r: &std::ops::Range<usize>) -> usize {
    let mut at = 0;
    v.chars()
        .filter(|c| {
            let inside = at >= r.start && at < r.end;
            at += c.len_utf16();
            inside
        })
        .map(char::len_utf8)
        .sum()
}

/// A pane's editing of one draft.
pub(crate) struct Editing {
    pub session: String,
    pub id: Ref,
    /// Its draft's key ([`Draft::key`]).
    pub key: u64,
    pub row: u64,
    /// The step or hunk under the cursor, by id.
    pub cursor: Option<ItemId>,
    pub field: Option<Field>,
    /// The first Esc with changes was pressed (§3.2).
    pub esc: bool,
    /// Its ring was painted in view since it began: keys act.
    pub shown: Rc<Cell<bool>>,
}

/// A pane's draft keys: the focus that holds them (made at the pane's
/// first frame, always drawn), the draft being edited, and the block's
/// note (why a take or a key did nothing), until the next input.
#[derive(Default)]
pub(crate) struct EditMode {
    pub focus: Option<FocusHandle>,
    pub at: Option<Editing>,
    pub note: Option<(u64, String)>,
    /// An Enter or an Esc in the open field, as its action came: the key,
    /// and whether it met a composition (read then: the input ends one at
    /// its Esc, and the platform may commit one before an Enter's
    /// `PressEnter`). Taken at its key-down (Esc) or its `PressEnter`.
    pub key: Option<(&'static str, bool)>,
    /// Its session followed to a new id with its field open
    /// ([`AppView::drafts_follow`]): the field closes at the pane's next
    /// frame, the draft keys kept.
    pub followed: bool,
}

impl EditMode {
    /// Whether its focus has the keys.
    pub(crate) fn holds(&self, window: &Window) -> bool {
        self.focus.as_ref().is_some_and(|f| f.is_focused(window))
    }

    /// Its open field's inputs.
    pub(crate) fn inputs(&self) -> (Option<&Entity<InputState>>, Option<&Entity<TextareaState>>) {
        let f = self.at.as_ref().and_then(|e| e.field.as_ref());
        (f.and_then(|f| f.line.as_ref()), f.and_then(|f| f.long.as_ref()))
    }
}

/// A `u-` id for a step the user adds: 30 bits of the OS's randomness
/// (the standard library's hash seeds), one `B` never named.
fn mint(d: &Draft) -> ItemId {
    loop {
        let r = std::collections::hash_map::RandomState::new().build_hasher().finish() as u32;
        let id = ItemId::mint(r);
        let named = d.base_block.has_item(&id) || d.base_block.removed_at(&id).is_some();
        let added = matches!(&d.intents, crate::drafts::Intents::Plan(p) if p.order.contains(&id));
        if !named && !added {
            return id;
        }
    }
}

/// The hash a painted item carries.
pub(crate) fn item_hash(id: &ItemId) -> u64 {
    crate::blocks::hash_of(id.as_str())
}

/// The items of `d`'s view the cursor walks: a plan's steps, a diff's
/// hunks.
fn items(b: &cards_spec::board::Block) -> Vec<ItemId> {
    match &b.body {
        Body::Plan(p) => p.steps.iter().map(|s| s.id.clone()).collect(),
        Body::Diff(d) => d.hunks.iter().map(|h| h.id.clone()).collect(),
        _ => Vec::new(),
    }
}

/// What field `of` edits, in block `b` (a draft's view).
/// Whether transcript `t` has row `row`.
fn has_row(t: &synaps_client::Transcript, row: u64) -> bool {
    t.items().iter().any(|i| i.id == row)
}

/// Transcript `t`'s row of call `call` (a tool row's: the row a block
/// anchors at, as a reattach numbers it).
fn row_of_call(t: &synaps_client::Transcript, call: &str) -> Option<u64> {
    t.items().iter().find(|i| i.tool.as_ref().is_some_and(|c| c.tool_id == call)).map(|i| i.id)
}

/// The call of transcript `t`'s row `row`, if a tool row.
fn call_of(t: &synaps_client::Transcript, row: u64) -> Option<std::sync::Arc<str>> {
    t.items().iter().find(|i| i.id == row).and_then(|i| i.tool.as_ref()).map(|c| std::sync::Arc::from(c.tool_id.as_str()))
}

fn field_value(b: &cards_spec::board::Block, of: &FieldOf) -> Option<String> {
    match (of, &b.body) {
        (FieldOf::Label(i), Body::Plan(p)) => p.steps.iter().find(|s| &s.id == i).map(|s| s.label.clone()),
        (FieldOf::Title, _) => Some(b.title.clone().unwrap_or_default()),
        (FieldOf::Text, Body::Note(n)) => Some(n.text.clone()),
        _ => None,
    }
}

/// The read-only words for the gate's reason (§3.1, design §3.2): whose
/// the session's input is in an exclusive session, and how to take it.
fn read_only(gate: &str, l: &crate::live::Live) -> String {
    if let Some(a) = l.active.as_ref()
        && a.input == synaps_client::ClientInput::Exclusive
        && a.mode != synaps_client::AttachMode::Observe
        && let Some(owner) = a.owner.filter(|o| *o != a.client)
    {
        return format!("edit from client #{owner} · /takeover");
    }
    gate.to_string()
}

impl AppView {
    /// The session of the pane acted on, attached.
    fn edit_session(&self) -> Option<String> {
        self.model.lives[self.panes.cur.live].active.as_ref().map(|a| a.session_id.clone())
    }

    /// The draft the pane edits.
    pub(crate) fn editing_draft(&self) -> Option<&Draft> {
        let e = self.panes.cur.edit.at.as_ref()?;
        self.block_drafts.by_key(e.key).map(|(_, d)| d)
    }

    fn editing_draft_mut(&mut self) -> Option<&mut Draft> {
        let k = self.panes.cur.edit.at.as_ref()?.key;
        self.block_drafts.by_key_mut(k)
    }

    /// The block's note in the pane acted on, until the next input.
    pub(crate) fn edit_says(&mut self, row: u64, note: impl Into<String>, cx: &mut Context<Self>) {
        let note = note.into();
        log::info!(target: "synaps_desktop::drafts", "draft {row}: {note}");
        self.panes.cur.edit.note = Some((row, note));
        self.remeasure_draft(row);
        cx.notify();
    }

    /// The draft drawn in block row `s`'s place in the pane acted on, if
    /// its session has one: held, or stale in its row.
    pub(crate) fn row_draft(&self, s: &Shown) -> Option<&Draft> {
        let session = self.model.lives[self.panes.cur.live].active.as_ref()?.session_id.as_str();
        self.block_drafts.in_row(session, &s.block.id)
    }

    /// Take block row `row`, painted as `tag`, then do `part` (module
    /// docs): `false` if nothing was taken, the block saying why.
    pub(crate) fn take_block(&mut self, row: u64, tag: Tag, part: Part, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.panes.cur.edit.note = None;
        let l = &self.model.lives[self.panes.cur.live];
        if let Err(gate) = l.input_gate() {
            let words = read_only(gate, l);
            self.edit_says(row, words, cx);
            return false;
        }
        let Some(session) = self.edit_session() else { return false };
        let Some(board) = self.bound_board() else { return false };
        let found = crate::lease::find_any(&board, row).filter(|(_, g)| g.is_none()).map(|(s, _)| s.clone());
        let Some(s) = found else {
            self.edit_says(row, crate::lease::CHANGED, cx);
            return false;
        };
        // A stale draft in its row (painted as its own incarnation, or the
        // row as the block's new one): its keys are Copy to composer and
        // Discard, and it takes no edit. It gives way to the new
        // incarnation's draft only once let go: its words are the user's.
        let now = s.bound.as_ref().map(|b| b.tag);
        let stale = self
            .block_drafts
            .in_row(&session, &s.block.id)
            .filter(|d| d.ended.is_some() && (d.tag == tag || now == Some(tag)))
            .map(|d| (d.id.clone(), d.key));
        let (id, key, first, rev) = match stale {
            Some((id, key)) => (id, key, None, None),
            None => {
                let Some(bound) = s.bound.clone().filter(|b| b.tag == tag) else {
                    self.edit_says(row, crate::lease::CHANGED, cx);
                    return false;
                };
                if !crate::drafts::editable(bound.kind) {
                    self.edit_says(row, NOT_EDITABLE, cx);
                    return false;
                }
                let Some(generation) = board.generation else { return false };
                // Where it is drawn now: where it stays while held (§3.4),
                // its rows named by their calls too.
                let t = &self.model.lives[self.panes.cur.live].transcript;
                let place = t.drawn().and_then(|d| crate::drafts::Place::of(d, &bound.id)).map(|p| p.called(|r| call_of(t, r)));
                match self.block_drafts.take(&session, &bound, generation) {
                    Ok(d) => {
                        if d.place.is_none() {
                            d.place = place;
                        }
                        (d.id.clone(), d.key, items(&d.view()).into_iter().next(), Some(bound.rev))
                    }
                    Err(why) => {
                        self.edit_says(row, why, cx);
                        return false;
                    }
                }
            }
        };
        if self.panes.cur.walk.walk.is_some() {
            self.walk_leave(false, window, cx);
        }
        if self.panes.cur.ask.entry.is_some() {
            self.leave_ask(false, window, cx);
        }
        let again = self.panes.cur.edit.at.as_ref().is_some_and(|e| e.key == key);
        if !again {
            self.edit_close_field(false, window, cx);
            self.panes.cur.edit.at =
                Some(Editing { session: session.clone(), id, key, row, cursor: first, field: None, esc: false, shown: Rc::default() });
            log::info!(
                target: "synaps_desktop::drafts",
                "draft {row}: taken at r{rev:?}; the desk holds {} drafts, {} bytes",
                self.block_drafts.len(),
                self.block_drafts.bytes()
            );
        }
        // The pre-check reads the bound board itself: its next job keeps
        // it ([`AppView::sync_drafts`]).
        self.sync_drafts(cx);
        let focus = self.panes.cur.edit.focus.get_or_insert_with(|| cx.focus_handle()).clone();
        window.focus(&focus, cx);
        if part != Part::Take {
            self.edit_part(part, window, cx);
        }
        self.remeasure_draft(row);
        cx.notify();
        true
    }

    /// The editing ends in the pane acted on; `refocus`: the keys go to its
    /// composer (never to the window, whose Esc denies a prompt).
    pub(crate) fn edit_leave(&mut self, refocus: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_close_field(false, window, cx);
        if let Some(e) = self.panes.cur.edit.at.take() {
            log::info!(target: "synaps_desktop::drafts", "draft {}: keys left", e.row);
            self.remeasure_draft(e.row);
        }
        if refocus {
            let c = self.panes.cur.composer.clone();
            c.update(cx, |s, cx| s.focus(window, cx));
        }
        cx.notify();
    }

    /// The draft the pane edits discarded (§3.2), the keys to the composer.
    fn edit_discard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(e) = self.panes.cur.edit.at.as_ref() {
            let (key, id) = (e.key, e.id.clone());
            self.block_drafts.discard_key(key);
            log::info!(target: "synaps_desktop::drafts", "draft of {id}: discarded");
        }
        self.edit_leave(true, window, cx);
        self.sync_drafts(cx);
    }

    /// Discard the draft of key `key` (a button's, the dock's), and every
    /// pane's editing of it.
    pub(crate) fn draft_discard(&mut self, key: u64, cx: &mut Context<Self>) {
        let row = self.block_drafts.by_key(key).map(|(_, d)| crate::blocks::row_id(&d.id));
        self.block_drafts.discard_key(key);
        if let Some(row) = row {
            self.remeasure_draft(row);
        }
        self.sync_drafts(cx);
        cx.notify();
    }

    /// Copy to composer (§3.2): line 1's words of the draft of key `key`
    /// into the pane's composer, after what it holds, and the draft
    /// discarded: its words are the user's now, to send as typed text.
    pub(crate) fn draft_copy(&mut self, key: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some((words, id)) = self.block_drafts.by_key(key).map(|(_, d)| (d.words(), d.id.clone())) else { return };
        let c = self.panes.cur.composer.clone();
        c.update(cx, |s, cx| {
            let had = s.value().to_string();
            let text = if had.trim().is_empty() { words.clone() } else { format!("{}\n{words}", had.trim_end()) };
            s.set_value(text, window, cx);
            s.focus(window, cx);
        });
        self.block_drafts.discard_key(key);
        self.remeasure_draft(crate::blocks::row_id(&id));
        if self.panes.cur.edit.at.as_ref().is_some_and(|e| e.key == key) {
            self.edit_close_field(false, window, cx);
            self.panes.cur.edit.at = None;
        }
        log::info!(target: "synaps_desktop::drafts", "draft of {id}: copied to the composer");
        self.sync_drafts(cx);
        cx.notify();
    }

    /// Part `part` of the draft the pane edits, as a key or a click does it.
    fn edit_part(&mut self, part: Part, window: &mut Window, cx: &mut Context<Self>) {
        let Some(e) = self.panes.cur.edit.at.as_ref() else { return };
        let (row, key) = (e.row, e.key);
        let Some(d) = self.editing_draft() else { return };
        let view = d.view();
        let by_hash = |h: u64| items(&view).into_iter().find(|i| item_hash(i) == h);
        if let Some(f) = d.ended
            && !matches!(part, Part::Copy | Part::Discard)
        {
            let says = f.says().unwrap_or_default();
            self.edit_says(row, says, cx);
            return;
        }
        let done: Result<(), &'static str> = match part {
            Part::Take => Ok(()),
            Part::Label(h) => match by_hash(h) {
                Some(i) => {
                    self.set_cursor(Some(i.clone()));
                    self.edit_open_field(FieldOf::Label(i), window, cx);
                    Ok(())
                }
                None => Err(crate::lease::CHANGED),
            },
            Part::Strike(h) => match by_hash(h) {
                Some(i) => {
                    self.set_cursor(Some(i.clone()));
                    self.editing_draft_mut().map_or(Ok(()), |d| d.strike(&i))
                }
                None => Err(crate::lease::CHANGED),
            },
            Part::Stop(h) => match by_hash(h) {
                Some(i) => {
                    self.set_cursor(Some(i.clone()));
                    self.editing_draft_mut().map_or(Ok(()), |d| d.stop(&i))
                }
                None => Err(crate::lease::CHANGED),
            },
            Part::Keep(h) | Part::Drop(h) => match by_hash(h) {
                Some(i) => {
                    self.set_cursor(Some(i.clone()));
                    self.editing_draft_mut().map_or(Ok(()), |d| d.hunk(&i, matches!(part, Part::Keep(_))))
                }
                None => Err(crate::lease::CHANGED),
            },
            Part::Text => {
                self.edit_open_field(FieldOf::Text, window, cx);
                Ok(())
            }
            Part::Title => {
                self.edit_open_field(FieldOf::Title, window, cx);
                Ok(())
            }
            Part::HandBack => Err(NOT_SENT),
            Part::Copy => {
                self.draft_copy(key, window, cx);
                return;
            }
            Part::Discard => {
                self.block_drafts.discard_key(key);
                self.edit_leave(true, window, cx);
                self.sync_drafts(cx);
                return;
            }
        };
        if let Some(e) = self.panes.cur.edit.at.as_mut() {
            e.esc = false;
        }
        if let Err(why) = done {
            self.edit_says(row, why, cx);
        }
        self.remeasure_draft(row);
        cx.notify();
    }

    fn set_cursor(&mut self, at: Option<ItemId>) {
        if let Some(e) = self.panes.cur.edit.at.as_mut() {
            e.cursor = at;
        }
    }

    /// A key down on the pane's draft focus: see the module.
    pub(crate) fn edit_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let m = &ks.modifiers;
        if m.control || m.alt || m.platform || m.function {
            return;
        }
        let Some(e) = self.panes.cur.edit.at.as_ref() else { return };
        let (shown, row) = (e.shown.get(), e.row);
        let key = ks.key.as_str();
        let text = ks.key_char.clone().filter(|c| !c.is_empty() && !c.chars().any(char::is_control));
        cx.stop_propagation();
        self.panes.cur.edit.note = None;
        let shifted = m.shift && matches!(key, "j" | "k");
        let ours = shifted
            || (!m.shift && matches!(key, "j" | "k" | "down" | "up" | "x" | "b" | "r" | "a" | "t" | "y" | "n" | "c" | "enter" | "escape"));
        if !shown {
            // Not painted yet: Esc and a printable key leave, the key typed
            // into the composer; nothing acts.
            if text.is_some() || key == "escape" {
                self.edit_leave(true, window, cx);
                if let Some(t) = text {
                    let c = self.panes.cur.composer.clone();
                    c.update(cx, |s, cx| EntityInputHandler::replace_text_in_range(s, None, &t, window, cx));
                }
            }
            return;
        }
        if !ours {
            if let Some(t) = text {
                self.edit_leave(true, window, cx);
                let c = self.panes.cur.composer.clone();
                c.update(cx, |s, cx| EntityInputHandler::replace_text_in_range(s, None, &t, window, cx));
            }
            return;
        }
        // Only a new, bare press acts (Shift+J/K: a new press, Shift alone).
        let fresh = if shifted { self.ask_presses.last } else { self.ask_presses.acts };
        if !fresh {
            log::debug!(target: "synaps_desktop::drafts", "draft key {key}: not a new, bare press");
            return;
        }
        let Some(d) = self.editing_draft() else {
            self.edit_leave(true, window, cx);
            return;
        };
        let kind = d.base_block.kind;
        let (changed, ended) = (d.changed(), d.ended.is_some());
        let view = d.view();
        let list = items(&view);
        let cursor = self.panes.cur.edit.at.as_ref().and_then(|e| e.cursor.clone());
        let at = cursor.as_ref().and_then(|c| list.iter().position(|i| i == c));
        let esc = self.panes.cur.edit.at.as_ref().is_some_and(|e| e.esc);
        if key != "escape"
            && let Some(e) = self.panes.cur.edit.at.as_mut()
        {
            e.esc = false;
        }
        use cards_spec::block::Kind as K;
        let hash = |i: &ItemId| item_hash(i);
        match (key, kind) {
            ("escape", _) => {
                // No changes: let go. Changes: the second Esc discards.
                if (!changed && !ended) || esc {
                    self.edit_discard(window, cx);
                } else if let Some(e) = self.panes.cur.edit.at.as_mut() {
                    e.esc = true;
                }
            }
            ("enter", _) if ended => self.edit_says(row, d_ended_words(&self.editing_draft()), cx),
            ("enter", _) => self.edit_part(Part::HandBack, window, cx),
            ("c", _) if ended => {
                if let Some(key) = self.panes.cur.edit.at.as_ref().map(|e| e.key) {
                    self.draft_copy(key, window, cx);
                }
            }
            (_, _) if ended => self.edit_says(row, d_ended_words(&self.editing_draft()), cx),
            ("j" | "down", K::Plan | K::Diff) if !shifted => {
                let next = at.map_or(0, |k| (k + 1).min(list.len().saturating_sub(1)));
                self.set_cursor(list.get(next).cloned());
            }
            ("k" | "up", K::Plan | K::Diff) if !shifted => {
                let next = at.map_or(0, |k| k.saturating_sub(1));
                self.set_cursor(list.get(next).cloned());
            }
            ("j" | "k", K::Plan) => {
                if let Some(c) = cursor.clone() {
                    let by = if key == "j" { 1 } else { -1 };
                    self.editing_draft_mut().map(|d| d.shift(&c, by));
                }
            }
            ("x", K::Plan) => {
                if let Some(c) = cursor.as_ref() {
                    self.edit_part(Part::Strike(hash(c)), window, cx);
                }
            }
            ("b", K::Plan) => {
                if let Some(c) = cursor.as_ref() {
                    self.edit_part(Part::Stop(hash(c)), window, cx);
                }
            }
            ("r", K::Plan) => {
                if let Some(c) = cursor.as_ref() {
                    self.edit_part(Part::Label(hash(c)), window, cx);
                }
            }
            ("a", K::Plan) => self.edit_add(cursor, window, cx),
            ("r", K::Note) => self.edit_part(Part::Text, window, cx),
            ("t", K::Note) => self.edit_part(Part::Title, window, cx),
            ("y", K::Diff) => {
                if let Some(c) = cursor.as_ref() {
                    self.edit_part(Part::Keep(hash(c)), window, cx);
                }
            }
            ("n", K::Diff) => {
                if let Some(c) = cursor.as_ref() {
                    self.edit_part(Part::Drop(hash(c)), window, cx);
                }
            }
            _ => {}
        }
        self.remeasure_draft(row);
        cx.notify();
    }

    /// `a`: a step added after `after`, its label in a field.
    fn edit_add(&mut self, after: Option<ItemId>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.panes.cur.edit.at.as_ref().map(|e| e.row) else { return };
        let Some(d) = self.editing_draft() else { return };
        let id = mint(d);
        match self.editing_draft_mut().map(|d| d.add(after.as_ref(), id.clone(), "")) {
            Some(Ok(())) => {
                self.set_cursor(Some(id.clone()));
                self.edit_open_field(FieldOf::Label(id), window, cx);
            }
            Some(Err(why)) => self.edit_says(row, why, cx),
            None => {}
        }
    }

    /// Open field `of` on the draft the pane edits, its value the draft's,
    /// with the keys (module docs).
    fn edit_open_field(&mut self, of: FieldOf, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_close_field(false, window, cx);
        let Some(key) = self.panes.cur.edit.at.as_ref().map(|e| e.key) else { return };
        // One field a draft on the desk: another pane's on it closes, what
        // it holds the draft's (module docs).
        let pane = self.panes.cur.id;
        let others: Vec<crate::pane::PaneId> = self
            .panes
            .iter()
            .filter(|p| p.id != pane && p.edit.at.as_ref().is_some_and(|e| e.key == key && e.field.is_some()))
            .map(|p| p.id)
            .collect();
        for o in others {
            self.in_pane(o, |this| this.edit_close_field(false, window, cx));
        }
        let Some(d) = self.editing_draft() else { return };
        let Some(value) = field_value(&d.view(), &of) else { return };
        let room = Rc::new(Cell::new(self.field_room(&value)));
        let refused = Rc::new(Cell::new(0u8));
        let seen = (value.len(), seen_hash(&value, &None));
        let field = if of == FieldOf::Text {
            let e = cx.new(|cx| {
                let mut s = TextareaState::new(window, cx).auto_grow(2, 6);
                s.set_value(value, window, cx);
                s
            });
            let sub = cx.subscribe_in(&e, window, move |this: &mut Self, e, ev: &InputEvent, window, cx| {
                if matches!(ev, InputEvent::Change) {
                    let v = e.read(cx).value().to_string();
                    this.in_pane(pane, |this| this.edit_typed(&v, window, cx));
                }
            });
            e.update(cx, |s, cx| s.focus(window, cx));
            let watch = cx.observe_in(&e, window, move |this: &mut Self, _, window, cx| {
                this.in_pane(pane, |this| this.edit_marked(window, cx));
            });
            Field { of, line: None, long: Some(e), room, refused, history: 0, seen, _sub: sub, _watch: watch }
        } else {
            // No text past its cap is taken, typed, pasted or marked: the
            // kit refuses it, and the block says why at the next frame.
            let (n, r, why, view) = (of.chars(), room.clone(), refused.clone(), cx.entity_id());
            let e = cx.new(|cx| {
                let mut s = InputState::new(window, cx).validate(move |text, cx: &mut App| {
                    let w = if text.chars().count() > n {
                        1
                    } else if text.len() > r.get() {
                        2
                    } else {
                        0
                    };
                    if w != 0 {
                        why.set(w);
                        // Said at the view's next frame.
                        cx.notify(view);
                    }
                    w == 0
                });
                s.set_value(value, window, cx);
                s
            });
            let sub = cx.subscribe_in(&e, window, move |this: &mut Self, e, ev: &InputEvent, window, cx| match ev {
                InputEvent::Change => {
                    let v = e.read(cx).value().to_string();
                    this.in_pane(pane, |this| this.edit_typed(&v, window, cx));
                }
                InputEvent::PressEnter { .. } => {
                    this.in_pane(pane, |this| this.edit_field_enter(window, cx));
                }
                _ => {}
            });
            e.update(cx, |s, cx| s.focus(window, cx));
            let watch = cx.observe_in(&e, window, move |this: &mut Self, _, window, cx| {
                this.in_pane(pane, |this| this.edit_marked(window, cx));
            });
            Field { of, line: Some(e), long: None, room, refused, history: 0, seen, _sub: sub, _watch: watch }
        };
        if let Some(ed) = self.panes.cur.edit.at.as_mut() {
            ed.field = Some(field);
        }
        cx.notify();
    }

    /// The bytes a field whose text is `value` may hold: those, and the
    /// draft's room.
    fn field_room(&self, value: &str) -> usize {
        let own = self.editing_draft().map_or(0, Draft::own_bytes);
        value.len() + crate::drafts::DRAFT_MAX_BYTES.saturating_sub(own)
    }

    /// The open field's text `v`, as typed: the draft's, as the draft took
    /// it. The field holds what the draft holds: cut or refused there, the
    /// field that was edited gets the draft's value back, in this event,
    /// the caret kept, and the block says why (module docs). The change is
    /// counted as the kit keeps it ([`Field::saw`]: a composition's commit
    /// at its marked text and the candidate), and its undo history is
    /// cleared once it may hold [`FIELD_HISTORY_MAX`].
    fn edit_typed(&mut self, v: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(e) = self.panes.cur.edit.at.as_ref() else { return };
        let Some(of) = e.field.as_ref().map(|f| f.of.clone()) else { return };
        let row = e.row;
        let held = |this: &Self| this.editing_draft().and_then(|d| field_value(&d.view(), &of)).unwrap_or_default();
        let seen = self.field_now(window, cx);
        let done = match (&of, self.editing_draft_mut()) {
            (FieldOf::Label(i), Some(d)) => d.rename(i, v),
            (FieldOf::Title, Some(d)) => d.note(true, v),
            (FieldOf::Text, Some(d)) => d.note(false, v),
            (_, None) => return,
        };
        let now = held(self);
        let room = self.field_room(&now);
        let says = match done {
            Err(why) => Some(why.to_string()),
            Ok(()) if now != v => Some(at_most(of.chars())),
            Ok(()) => None,
        };
        let set = match self.panes.cur.edit.at.as_mut().and_then(|e| e.field.as_mut()) {
            Some(f) => {
                f.room.set(room);
                if let Some((fv, marked)) = seen {
                    f.saw(&fv, &marked);
                }
                now != v || f.history > FIELD_HISTORY_MAX
            }
            None => false,
        };
        if set {
            self.field_set(&now, window, cx);
        }
        if let Some(why) = says {
            self.edit_says(row, why, cx);
        }
        self.remeasure_draft(row);
        cx.notify();
    }

    /// The open field's text set to `value`, the caret where it was (at
    /// most at the end), its undo history cleared (the kit's `set_value`).
    fn field_set(&mut self, value: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.panes.cur.edit.at.as_mut().and_then(|e| e.field.as_mut()) else { return };
        f.cleared(value);
        let (line, long) = (f.line.clone(), f.long.clone());
        if let Some(l) = line {
            l.update(cx, |s, cx| {
                let at = s.cursor().min(value.len());
                s.set_value(value.to_string(), window, cx);
                s.set_selected_range(at..at, cx);
            });
        }
        if let Some(a) = long {
            a.update(cx, |s, cx| {
                let at = s.cursor().min(value.len());
                s.set_value(value.to_string(), window, cx);
                s.set_selected_range(at..at, cx);
            });
        }
    }

    /// The open field's text and its marked range (UTF-16, the kit's).
    fn field_now(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<(String, Option<std::ops::Range<usize>>)> {
        let f = self.panes.cur.edit.at.as_ref()?.field.as_ref()?;
        match (f.line.clone(), f.long.clone()) {
            (Some(l), _) => Some((l.read(cx).value().to_string(), l.update(cx, |s, cx| s.marked_text_range(window, cx)))),
            (None, Some(a)) => Some((a.read(cx).value().to_string(), a.update(cx, |s, cx| s.marked_text_range(window, cx)))),
            (None, None) => None,
        }
    }

    /// The open field notified (an IME marks text in it, or any change):
    /// the kit sends no `Change` for marked text, checks only a one-line
    /// field's, and keeps each preedit in its undo history (gpui-base
    /// 0.9.1, `replace_and_mark_text_in_range`), so each is checked here
    /// as it comes, counted as the kit keeps it ([`Field::saw`]: the text
    /// it took out and the text it marked; the commit or cancel that ends
    /// it, at the marked text it took out). Marked text past the field's
    /// characters (a text area's are within the draft's room, asserted
    /// with [`COMPOSED_LONG`]; a line's past either the kit refuses), or a
    /// composition whose history would pass [`FIELD_HISTORY_MAX`], is
    /// refused: the field gets the draft's value back, which ends the
    /// composition and clears its history, and the block says why. A
    /// composition's commit is a `Change` ([`AppView::edit_typed`]).
    fn edit_marked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((v, marked)) = self.field_now(window, cx) else { return };
        let Some(e) = self.panes.cur.edit.at.as_mut() else { return };
        let row = e.row;
        let Some(f) = e.field.as_mut() else { return };
        if !f.saw(&v, &marked) {
            return;
        }
        let Some(mark) = marked.as_ref().map(|r| utf16_bytes(&v, r)) else { return };
        let of = f.of.clone();
        let why = if v.chars().count() > of.chars() {
            at_most(of.chars())
        } else if f.history > FIELD_HISTORY_MAX {
            COMPOSED_LONG.to_string()
        } else {
            return;
        };
        let held = self.editing_draft().and_then(|d| field_value(&d.view(), &of)).unwrap_or_default();
        log::info!(target: "synaps_desktop::drafts", "draft {row}: a composition refused ({} bytes marked): {why}", mark);
        self.field_set(&held, window, cx);
        self.edit_says(row, why, cx);
        self.remeasure_draft(row);
    }

    /// The open field closes; `refocus`: the block's keys have the focus
    /// again. An added step left with no label is taken out.
    pub(crate) fn edit_close_field(&mut self, refocus: bool, window: &mut Window, cx: &mut Context<Self>) {
        // What a composition left (it ended with no `Change`), the draft's.
        self.edit_committed(window, cx);
        let Some(f) = self.panes.cur.edit.at.as_mut().and_then(|e| e.field.take()) else { return };
        if let FieldOf::Label(i) = &f.of
            && let Some(d) = self.editing_draft_mut()
            && let crate::drafts::Intents::Plan(p) = &d.intents
            && p.added.iter().any(|(x, l)| x == i && l.trim().is_empty())
        {
            let _ = d.strike(i);
        }
        if let Some(row) = self.panes.cur.edit.at.as_ref().map(|e| e.row) {
            self.remeasure_draft(row);
        }
        if refocus && let Some(focus) = self.panes.cur.edit.focus.clone() {
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    /// Enter in the open one-line field (its `PressEnter`): back to the
    /// block's keys, on a new, bare press that met no composition (D8b:
    /// a one-line field's Enter hands back, §3.2).
    fn edit_field_enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let enter = self.ask_presses.field.take();
        let met = self.panes.cur.edit.key.take_if(|k| k.0 == "enter");
        if enter.and_then(|f| f.acts) != Some(true) || met != Some(("enter", false)) || self.composing_now(window, cx) {
            log::debug!(target: "synaps_desktop::drafts", "field Enter {enter:?} {met:?}: not a new, bare press, or a composition's");
            return;
        }
        self.edit_close_field(true, window, cx);
    }

    /// The open field's text, the draft's, once no composition is under
    /// way: marked text sends no `Change`, nor does a composition ended in
    /// place (its Esc), so what it left is read here.
    fn edit_committed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.panes.cur.edit.at.as_ref().and_then(|e| e.field.as_ref()) else { return };
        let v = match (&f.line, &f.long) {
            (Some(l), _) => l.read(cx).value().to_string(),
            (None, Some(a)) => a.read(cx).value().to_string(),
            (None, None) => return,
        };
        if self.composing_now(window, cx) {
            return;
        }
        self.edit_typed(&v, window, cx);
    }

    /// An Enter or an Esc (`key`) as its action comes, in the open field
    /// only: whether it met a composition.
    fn edit_field_action(&mut self, key: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let (a, l) = self.panes.cur.edit.inputs();
        let focused = a.is_some_and(|e| e.read(cx).focus_handle(cx).is_focused(window))
            || l.is_some_and(|e| e.read(cx).focus_handle(cx).is_focused(window));
        self.panes.cur.edit.key = focused.then(|| (key, self.composing_now(window, cx)));
    }

    /// Esc or Tab in the open field, at its key-down: back to the block's
    /// keys, on a new, bare press that met no composition.
    fn edit_field_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let (a, l) = self.panes.cur.edit.inputs();
        let focused = a.is_some_and(|e| e.read(cx).focus_handle(cx).is_focused(window))
            || l.is_some_and(|e| e.read(cx).focus_handle(cx).is_focused(window));
        if !focused {
            return;
        }
        let key = ev.keystroke.key.as_str();
        let esc = if key == "escape" { self.panes.cur.edit.key.take_if(|k| k.0 == "escape").map(|k| k.1) } else { None };
        cx.stop_propagation();
        // Esc: its action met no composition, and its key-down is a new,
        // bare press (the root stamped it before this capture).
        let fresh = match key {
            "escape" => esc == Some(false) && self.ask_presses.acts,
            _ => self.ask_presses.acts,
        };
        if !fresh || self.composing_now(window, cx) {
            log::debug!(target: "synaps_desktop::drafts", "field {key}: not a new, bare press, or a composition's");
            self.edit_committed(window, cx);
            return;
        }
        self.edit_close_field(true, window, cx);
    }

    /// As a pane's frame begins: its draft's session judged on its bound
    /// board (what ended makes its draft stale or an orphan), the bound
    /// board kept for the pre-check while a draft is held, and an editing
    /// whose draft went, or whose keys went elsewhere, ended.
    pub(crate) fn sync_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.panes.cur.edit.focus.is_none() {
            self.panes.cur.edit.focus = Some(cx.focus_handle());
        }
        self.sync_drafts(cx);
        // Followed to a new id: the rows are drawn again from the new
        // attachment's history, the open field's too, and a frame without
        // it loses its focus to the window. So the field closes, its text
        // the draft's, and the draft keys stay (`r` opens it again).
        if std::mem::take(&mut self.panes.cur.edit.followed) {
            let (a, l) = self.panes.cur.edit.inputs();
            let in_field = a.is_some_and(|x| x.read(cx).focus_handle(cx).is_focused(window))
                || l.is_some_and(|x| x.read(cx).focus_handle(cx).is_focused(window));
            self.edit_close_field(in_field, window, cx);
        }
        let Some(e) = self.panes.cur.edit.at.as_ref() else {
            if self.panes.cur.edit.holds(window) {
                self.edit_leave(true, window, cx);
            }
            return;
        };
        // What the open one-line field refused, said (module docs).
        if let Some((why, n, row)) = e.field.as_ref().map(|f| (f.refused.replace(0), f.of.chars(), e.row)).filter(|w| w.0 != 0) {
            self.edit_says(row, if why == 1 { at_most(n) } else { crate::drafts::DRAFT_FULL.to_string() }, cx);
        }
        let Some(e) = self.panes.cur.edit.at.as_ref() else { return };
        let here = self.edit_session().as_deref() == Some(e.session.as_str()) && self.model.tabs.view().is_none();
        // A draft not drawn in its row (an orphan, one another draft of its
        // block has the row of): its keys are the dock's buttons.
        let have = self.block_drafts.by_key(e.key).is_some_and(|(s, d)| s == e.session && self.block_drafts.rowed(s, d));
        let (a, l) = self.panes.cur.edit.inputs();
        let in_field = a.is_some_and(|x| x.read(cx).focus_handle(cx).is_focused(window))
            || l.is_some_and(|x| x.read(cx).focus_handle(cx).is_focused(window));
        let keys = self.panes.cur.edit.holds(window) || in_field;
        if !(here && have) {
            self.edit_leave(keys, window, cx);
        } else if !keys {
            // The keys went elsewhere (a click on the composer, another
            // pane): the draft stays held, the editing ends.
            self.edit_leave(false, window, cx);
        }
    }

    /// The pane's session's drafts judged on its bound board (module docs
    /// of [`crate::drafts`]), and each store's jobs keeping the bound board
    /// while its session holds a draft (a job asked for when that changes,
    /// or where its held blocks are kept). A session followed to a new id
    /// takes its drafts first. A draft whose footer would say another thing
    /// now is measured again, in every pane.
    pub(crate) fn sync_drafts(&mut self, cx: &mut Context<Self>) {
        for (from, to) in self.model.tabs.take_followed() {
            self.drafts_follow(&from, &to, cx);
        }
        for live in self.model.lives.ids() {
            let l = &self.model.lives[live];
            let Some(session) = l.active.as_ref().map(|a| a.session_id.clone()) else { continue };
            let board = l.board.waiting.clone().or_else(|| l.transcript.drawn().cloned());
            let moved = match board.as_deref() {
                Some(d) if d.epoch == l.transcript.blocks_epoch() => self.block_drafts.judge(&session, d),
                // No board at all and no block call: every block is gone.
                None if !l.transcript.has_blocks() => self.block_drafts.gone(&session),
                _ => false,
            };
            // A held block whose row its history numbered anew (a
            // reattach, a cold reload): its place there, its row found by
            // its call (§3.4: the anchor it was taken at, the agent's move
            // of it still waiting), whatever row has its number now. No
            // row has that call (the anchor itself is gone, not
            // renumbered): held where the board draws it now.
            let t = &l.transcript;
            for (id, p) in self.block_drafts.places(&session) {
                if p.there(|r| has_row(t, r), |c| row_of_call(t, c)) {
                    continue;
                }
                let now = t.drawn().filter(|d| d.epoch == t.blocks_epoch());
                match p.renumbered(|r| has_row(t, r), |c| row_of_call(t, c)) {
                    Some(q) => self.block_drafts.re_place(&session, &id, Some(q)),
                    None if now.is_some() => {
                        let q = now.and_then(|d| crate::drafts::Place::of(d, &id)).map(|q| q.called(|r| call_of(t, r)));
                        log::info!(target: "synaps_desktop::drafts", "draft of {id}: its anchor's call is gone; held where it is drawn");
                        self.block_drafts.re_place(&session, &id, q);
                    }
                    None => {}
                }
            }
            let want = self.block_drafts.holds(&session);
            let places = {
                use std::hash::{Hash as _, Hasher as _};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                self.block_drafts.places(&session).hash(&mut h);
                h.finish()
            };
            let l = &mut self.model.lives[live];
            if l.transcript.blocks_keeps_bound() != want || l.board.places != places {
                // Another job: the bound board kept or let go; a block let
                // go drawn where the agent put it.
                l.transcript.blocks_keep_bound(want);
                l.board.places = places;
                l.board.again();
            }
            let mut rows: Vec<u64> =
                if moved { self.block_drafts.of(&session).map(|d| crate::blocks::row_id(&d.id)).collect() } else { Vec::new() };
            rows.extend(self.draft_feet(live, &session));
            if !rows.is_empty() {
                for row in rows {
                    self.remeasure_draft(row);
                }
                cx.notify();
            }
        }
    }

    /// Session `session`'s held drafts whose footer says another thing on
    /// store `live`'s bound board now than when last judged (the pre-check
    /// arrived, or changed: its lines are its height), by row. Judged once
    /// a board ([`crate::board::Work::landed`]) and a change.
    fn draft_feet(&mut self, live: crate::live::LiveId, session: &str) -> Vec<u64> {
        use std::hash::{Hash as _, Hasher as _};
        let l = &self.model.lives[live];
        let st = l.board.waiting.clone().or_else(|| l.transcript.drawn().cloned()).and_then(|d| d.board.clone());
        let landed = l.board.landed;
        let mut rows = Vec::new();
        for d in self.block_drafts.of_mut(session) {
            let memo = (landed, d.serial);
            if d.foot.is_some_and(|f| f.0 == memo) {
                continue;
            }
            let mut h = std::collections::hash_map::DefaultHasher::new();
            match (d.ended, st.as_deref()) {
                (Some(f), _) => f.says().hash(&mut h),
                (None, None) => "checking".hash(&mut h),
                (None, Some(st)) => {
                    let c = d.check(st);
                    (c.footer(), c.off, c.message.is_some(), d.agent_changes(st)).hash(&mut h);
                }
            }
            let look = h.finish();
            if d.foot.is_some_and(|f| f.1 != look) {
                rows.push(crate::blocks::row_id(&d.id));
            }
            d.foot = Some((memo, look));
        }
        rows
    }

    /// Session `from` followed to a new id `to` (the same conversation): its
    /// drafts are `to`'s ([`crate::drafts::Drafts::follow`]), and so is
    /// every pane's editing of them, its open field and text kept.
    pub(crate) fn drafts_follow(&mut self, from: &str, to: &str, cx: &mut Context<Self>) {
        let gone = self.block_drafts.follow(from, to);
        for p in self.panes.iter_mut() {
            if let Some(e) = p.edit.at.as_mut().filter(|e| e.session == from && !gone.contains(&e.key)) {
                e.session = to.to_string();
                p.edit.followed |= e.field.is_some();
            }
        }
        let rows: Vec<u64> = self.block_drafts.of(to).map(|d| crate::blocks::row_id(&d.id)).collect();
        for row in rows {
            self.remeasure_draft(row);
        }
        cx.notify();
    }

    /// Board `d` for store `live` as this desk draws it: the blocks its
    /// session's drafts are drawn in, where they were taken (§3.4,
    /// [`crate::drafts::hold_places`]).
    pub(crate) fn drafts_in_place(
        &self,
        live: crate::live::LiveId,
        d: std::sync::Arc<synaps_client::blocks::Drawn>,
    ) -> std::sync::Arc<synaps_client::blocks::Drawn> {
        let l = &self.model.lives[live];
        let Some(session) = l.active.as_ref().map(|a| a.session_id.as_str()) else { return d };
        // Its rows found by their calls (the session's transcript's: the
        // board's rows, a watch focused or not), numbered anew or not. Its
        // call gone: drawn where the agent put it, taken as its place
        // ([`AppView::sync_drafts`]).
        let t = &l.transcript;
        let holds: Vec<_> = self
            .block_drafts
            .places(session)
            .into_iter()
            .filter_map(|(id, p)| p.renumbered(|r| has_row(t, r), |c| row_of_call(t, c)).map(|q| (id, q)))
            .collect();
        if holds.is_empty() {
            return d;
        }
        match crate::drafts::hold_places(&d, &holds) {
            Some(x) => std::sync::Arc::new(x),
            None => d,
        }
    }

    /// The pane's draft hooks, on its element: the focus that holds the
    /// draft keys (zero-sized, always drawn once made), and the open
    /// field's Enter, Esc and Tab (an entered card's [`FieldKey`] rule).
    pub(crate) fn edit_hooks(&self, d: Div, cx: &mut Context<Self>) -> Div {
        let pane = self.panes.cur.id;
        let focus = self.panes.cur.edit.focus.clone().map(|f| {
            div().absolute().size_0().track_focus(&f).key_context(EDIT_CTX).on_key_down(cx.listener(
                move |this, ev: &KeyDownEvent, window, cx| {
                    this.in_pane(pane, |this| this.edit_key(ev, window, cx));
                },
            ))
        });
        d.children(focus)
            // An Enter or an Esc in the open field, as its action comes:
            // whether it met a composition, read before the input ends or
            // commits one.
            .capture_action(cx.listener(move |this, _: &gpui_kit::base::input::Enter, window, cx| {
                this.in_pane(pane, |this| this.edit_field_action("enter", window, cx));
            }))
            .capture_action(cx.listener(move |this, _: &gpui_kit::base::input::Escape, window, cx| {
                this.in_pane(pane, |this| this.edit_field_action("escape", window, cx));
            }))
            .capture_key_down(cx.listener(move |this, ev: &KeyDownEvent, window, cx| {
                if matches!(ev.keystroke.key.as_str(), "escape" | "tab") {
                    this.in_pane(pane, |this| this.edit_field_key(ev, window, cx));
                }
            }))
    }

    /// Block row `row`, drawn as a draft, measured again in every pane that
    /// shows it (its draft is the session's: each pane draws it).
    pub(crate) fn remeasure_draft(&mut self, row: u64) {
        for pane in self.panes.iter_mut() {
            if let Some(ix) = pane.disp.iter().position(|d| d.id == row && matches!(d.kind, crate::turns::Kind::Block { .. })) {
                pane.list.remeasure_items(ix..ix + 1);
            }
        }
    }

    // ── drawn ─────────────────────────────────────────────────────────

    /// Block row `id` drawn as its session's draft `d` (module docs): the
    /// draft over `B`, its header, its editors, its footer.
    pub(crate) fn render_draft(&self, d: &Draft, id: u64, p: &Palette, m: &Metrics, cx: &mut Context<Self>) -> AnyElement {
        let g = crate::blocks::Geo::of(m);
        let pane = self.panes.cur.id;
        let view = d.view();
        let tag = d.tag;
        let editing = self.panes.cur.edit.at.as_ref().filter(|e| e.row == id && e.key == d.key);
        // The steps this draft added (a bound `u-` step is `B`'s).
        let added = |i: &ItemId| matches!(&d.intents, crate::drafts::Intents::Plan(p) if p.adds(i));
        let cursor = editing.and_then(|e| e.cursor.clone());
        let field = editing.and_then(|e| e.field.as_ref());
        let board = self.bound_board();
        let st = board.as_deref().and_then(|b| b.board.clone());
        let ended = d.ended;
        let agent = st.as_deref().map_or(0, |st| d.agent_changes(st));
        let head_words = match ended.and_then(Fate::says) {
            Some(w) => w,
            None if agent > 0 => format!("EDITING · r{} · {agent} agent change{}", d.base, if agent == 1 { "" } else { "s" }),
            None => format!("EDITING · r{}", d.base),
        };
        let title = crate::ask::shown(view.title.as_deref().unwrap_or(&d.id.to_string()), cards_spec::Lines::Single);
        let ink = if ended.is_some() { p.dim_t } else { p.focus };
        let hw = head_words.clone();
        let press =
            |part: Part| crate::lease::Press { at: crate::lease::Target { row: id, tag }, act: crate::lease::Act::Edit(part), group: None };
        let clicks = |el: Stateful<Div>, part: Part| {
            let pr = press(part);
            el.cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.block_down(pane, pr, cx);
                    }),
                )
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseUpEvent, window, cx| {
                        cx.stop_propagation();
                        this.in_pane(pane, |this| this.draft_up(id, part, window, cx));
                    }),
                )
        };
        let says = self.panes.cur.edit.note.as_ref().filter(|n| n.0 == id).map(|n| n.1.clone());
        let head = h_flex()
            .relative()
            .h(px(g.head))
            .w_full()
            .gap(px(g.gap))
            .items_center()
            .whitespace_nowrap()
            .font_family(fonts::SANS)
            .text_px(m.label)
            .debug_selector(move || format!("draft-head {id} {hw}"))
            .child(
                div()
                    .min_w_0()
                    .flex_shrink(1.)
                    .overflow_hidden()
                    .text_ellipsis()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(p.prose_t)
                    .child(title),
            )
            .child(div().flex_1().min_w(px(8.)))
            .child(div().flex_none().font_family(fonts::MONO).text_px(m.meta).text_color(ink).child(head_words));
        let mut body = v_flex().w_full().pb(px((6. * m.r).round()));
        let ran = |s: StepState| matches!(s, StepState::Doing | StepState::Done | StepState::Failed);
        match &view.body {
            Body::Plan(pl) => {
                for s in &pl.steps {
                    let h = item_hash(&s.id);
                    let at = cursor.as_ref() == Some(&s.id);
                    let sid = s.id.to_string();
                    let open = field.filter(|f| f.of == FieldOf::Label(s.id.clone()));
                    let state = s.state.as_str();
                    let stop = if s.stop { " stop" } else { "" };
                    let label_t = s.label.clone();
                    let gutter = div()
                        .id(("draft-stop", h ^ id))
                        .flex_none()
                        .w(px(10.))
                        .h_full()
                        .when(s.stop, |d| d.child(div().w(px(3.)).h_full().bg(p.warn)))
                        .debug_selector(move || format!("draft-stop-gutter {id} {h}"));
                    let gutter = if ran(s.state) || added(&s.id) {
                        gutter.into_any_element()
                    } else {
                        clicks(gutter, Part::Stop(h)).into_any_element()
                    };
                    let glyph = div()
                        .id(("draft-strike", h ^ id))
                        .flex_none()
                        .w(px(16.))
                        .font_family(fonts::MONO)
                        .text_px(m.meta)
                        .text_color(if s.state == StepState::Skipped { p.dim_t } else { p.prose_t })
                        .child(match s.state {
                            StepState::Todo => "○",
                            StepState::Doing => "◐",
                            StepState::Done => "●",
                            StepState::Failed => "✕",
                            StepState::Skipped => "–",
                        })
                        .debug_selector(move || format!("draft-strike-gutter {id} {h}"));
                    let label: AnyElement = match open {
                        Some(f) => div()
                            .flex_1()
                            .min_w_0()
                            .key_context(EDIT_CTX)
                            .debug_selector(move || format!("draft-field {id}"))
                            .children(f.line.as_ref().map(|e| Input::new(e).appearance(false).text_px(m.label).text_color(p.ink)))
                            .into_any_element(),
                        None => clicks(
                            div()
                                .id(("draft-label", h ^ id))
                                .min_w_0()
                                .flex_shrink(1.)
                                .overflow_hidden()
                                .text_ellipsis()
                                .text_color(if s.state == StepState::Skipped { p.dim_t } else { p.prose_t })
                                .when(s.state == StepState::Skipped, |d| d.line_through())
                                .child(crate::ask::shown(&s.label, cards_spec::Lines::Single)),
                            Part::Label(h),
                        )
                        .into_any_element(),
                    };
                    body = body.child(
                        h_flex()
                            .h(px(g.step))
                            .w_full()
                            .gap(px(g.gap))
                            .items_center()
                            .whitespace_nowrap()
                            .font_family(fonts::SANS)
                            .text_px(m.label)
                            .when(at, |d| d.bg(p.well_d))
                            .debug_selector(move || format!("draft-step {id} {sid} {state}{stop}{} {label_t}", if at { " at" } else { "" }))
                            .child(gutter)
                            .child(if added(&s.id) { glyph.into_any_element() } else { clicks(glyph, Part::Strike(h)).into_any_element() })
                            .child(label)
                            .child(div().flex_1().min_w(px(8.)))
                            .when(s.stop, |d| d.child(div().flex_none().text_px(m.meta).text_color(p.warn).child("stop")))
                            .when(added(&s.id), |d| d.child(div().flex_none().text_px(m.meta).text_color(p.dim_t).child("added")))
                            .child(div().flex_none().text_px(m.meta).text_color(p.dim_t).child(s.state.as_str())),
                    );
                }
            }
            Body::Note(n) => {
                let title_open = field.filter(|f| f.of == FieldOf::Title);
                let text_open = field.filter(|f| f.of == FieldOf::Text);
                let title_t = view.title.clone().unwrap_or_default();
                body = body.child(match title_open.and_then(|f| f.line.as_ref()) {
                    Some(e) => div()
                        .w_full()
                        .key_context(EDIT_CTX)
                        .debug_selector(move || format!("draft-field {id}"))
                        .child(Input::new(e).appearance(false).text_px(m.label).text_color(p.ink))
                        .into_any_element(),
                    None => clicks(
                        div()
                            .id(("draft-title", id))
                            .w_full()
                            .h(px(g.line))
                            .font_family(fonts::SANS)
                            .text_px(m.meta)
                            .text_color(p.dim_t)
                            .debug_selector(move || format!("draft-title {id} {title_t}"))
                            .child(if view.title.is_some() { "title · t" } else { "add a title · t" }),
                        Part::Title,
                    )
                    .into_any_element(),
                });
                let text_t = n.text.clone();
                body = body.child(match text_open.and_then(|f| f.long.as_ref()) {
                    Some(e) => div()
                        .w_full()
                        .key_context(EDIT_CTX)
                        .debug_selector(move || format!("draft-field {id}"))
                        .child(Textarea::new(e).appearance(false).text_px(m.label).text_color(p.ink))
                        .into_any_element(),
                    None => clicks(
                        div()
                            .id(("draft-text", id))
                            .w_full()
                            .font_family(fonts::SANS)
                            .text_px(m.label)
                            .line_height(px(g.line))
                            .text_color(p.prose_t)
                            .debug_selector(move || format!("draft-text {id} {text_t}"))
                            .child(crate::ask::shown(&n.text, cards_spec::Lines::Multi)),
                        Part::Text,
                    )
                    .into_any_element(),
                });
            }
            Body::Diff(df) => {
                for hk in &df.hunks {
                    let h = item_hash(&hk.id);
                    let at = cursor.as_ref() == Some(&hk.id);
                    let hid = hk.id.to_string();
                    let state = hk.state.as_str();
                    let button = |label: &'static str, part: Part, on: bool| {
                        clicks(
                            div()
                                .id((label, h ^ id))
                                .flex_none()
                                .px(px(6.))
                                .font_family(fonts::MONO)
                                .text_px(m.meta)
                                .text_color(if on { p.focus } else { p.dim_t })
                                .when(on, |d| d.bg(p.well_d))
                                .debug_selector(move || format!("draft-{label} {id} {h}"))
                                .child(label),
                            part,
                        )
                    };
                    body = body.child(
                        h_flex()
                            .h(px(g.step))
                            .w_full()
                            .gap(px(g.gap))
                            .items_center()
                            .whitespace_nowrap()
                            .font_family(fonts::MONO)
                            .text_px(m.meta)
                            .when(at, |d| d.bg(p.well_d))
                            .debug_selector(move || format!("draft-hunk {id} {hid} {state}{}", if at { " at" } else { "" }))
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_shrink(1.)
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .text_color(p.prose_t)
                                    .child(format!("@@ {}", hk.at.as_deref().unwrap_or(hk.id.as_str()))),
                            )
                            .child(div().flex_1().min_w(px(8.)))
                            .child(button("keep", Part::Keep(h), hk.state == cards_spec::block::HunkState::Kept))
                            .child(button("drop", Part::Drop(h), hk.state == cards_spec::block::HunkState::Dropped)),
                    );
                }
            }
            _ => {}
        }
        // The footer (§3.2): what fails now, why nothing goes, the Esc,
        // the hold's word, the buttons.
        let checked = match ended {
            Some(_) => None,
            None => st.as_deref().map(|st| d.check(st)),
        };
        let mut lines: Vec<(String, Hsla)> = Vec::new();
        if let Some(f) = checked.as_ref().and_then(crate::drafts::Checked::footer) {
            lines.push((f, p.warn));
        }
        if ended.is_none() {
            let off = match &checked {
                None => Some("checking…".to_string()),
                Some(c) => c.off.clone(),
            };
            if let Some(o) = off {
                lines.push((o, p.dim_t));
            }
            if editing.is_some_and(|e| e.esc) {
                lines.push((ESC_AGAIN.into(), p.focus));
            }
            lines.push((KEEPS_WORKING.into(), p.dim_t));
        }
        if let Some(n) = says {
            lines.push((n, p.acc));
        }
        let can = checked.as_ref().is_some_and(|c| c.message.is_some());
        let button = |label: &'static str, part: Part, on: bool, ready: bool| {
            clicks(
                div()
                    .id((label, id))
                    .flex_none()
                    .px(px(8.))
                    .h(px((22. * m.r).round()))
                    .flex()
                    .items_center()
                    .font_family(fonts::SANS)
                    .text_px(m.meta)
                    .text_color(if on { p.prose_t } else { p.dim_t })
                    .bg(p.well_d)
                    .debug_selector(move || {
                        format!("draft-button {id} {label} {}{}", if on { "on" } else { "off" }, if ready { " ready" } else { "" })
                    })
                    .child(label),
                part,
            )
        };
        let buttons = match ended {
            Some(_) => h_flex().gap(px(g.gap)).child(button("Copy to composer", Part::Copy, true, false)).child(button(
                "Discard",
                Part::Discard,
                true,
                false,
            )),
            // D8b sends it: until then it is off, whatever the pre-check
            // says (`ready`: the pre-check left something to hand back).
            None => h_flex().gap(px(g.gap)).child(button("Hand back", Part::HandBack, false, can)).child(button(
                "Discard",
                Part::Discard,
                true,
                false,
            )),
        };
        let footer = v_flex()
            .w_full()
            .gap(px(2.))
            .pb(px((6. * m.r).round()))
            .children(lines.into_iter().map(|(t, c)| {
                let tt = t.clone();
                div()
                    .w_full()
                    .font_family(fonts::SANS)
                    .text_px(m.meta)
                    .text_color(c)
                    .whitespace_normal()
                    .debug_selector(move || format!("draft-footer {id} {tt}"))
                    .child(t)
            }))
            .child(buttons);
        let shown = editing.map(|e| e.shown.clone());
        let el = div()
            .id(("block", id))
            .debug_selector(move || format!("block {id}"))
            .relative()
            .w_full()
            .bg(p.card_d)
            .pl(px(g.spine + g.pad))
            .pr(px(g.pad))
            .child(div().absolute().left_0().top_0().bottom_0().w(px(g.spine)).bg(ink).debug_selector(move || format!("draft-spine {id}")))
            .child(div().absolute().size_0().debug_selector(move || format!("block-draft {id}")))
            .child(head)
            .child(body)
            .child(footer)
            .child(crate::lease::probe(&self.panes.cur.lease, &self.fx, id, tag, id))
            // The entered card's focus ring, painted: its keys act.
            .children(shown.map(|shown| {
                div().absolute().inset_0().border_1().border_color(p.focus).debug_selector(move || format!("draft-ring {id}")).child(
                    canvas(
                        |_, _, _| {},
                        move |bounds, (), window, _| {
                            let in_view = bounds.intersect(&window.content_mask().bounds);
                            if in_view.size.width > px(0.) && in_view.size.height > px(0.) {
                                shown.set(true);
                            }
                        },
                    )
                    .absolute()
                    .inset_0(),
                )
            }));
        el.into_any_element()
    }

    /// Pointer-up on part `part` of draft row `row`: the press's target
    /// acts only if it is this one and still holds as painted (its draft,
    /// and for content its incarnation on the bound board); else nothing,
    /// and the note.
    pub(crate) fn draft_up(&mut self, row: u64, part: Part, window: &mut Window, cx: &mut Context<Self>) {
        let l = &mut self.panes.cur.lease;
        l.pressed = false;
        let Some(down) = l.down.take().filter(|d| d.at.row == row && d.act == crate::lease::Act::Edit(part)) else { return };
        let Some(session) = self.edit_session() else { return };
        let Some(board) = self.bound_board() else { return };
        let Some(s) = crate::lease::find_any(&board, row).filter(|(_, g)| g.is_none()).map(|(s, _)| s.clone()) else {
            self.edit_says(row, crate::lease::CHANGED, cx);
            return;
        };
        let held = self.block_drafts.in_row(&session, &s.block.id).filter(|d| d.tag == down.at.tag).map(|d| (d.key, d.tag));
        match held {
            Some((key, tag)) => {
                // Its keys come here: the block's.
                let editing = self.panes.cur.edit.at.as_ref().is_some_and(|e| e.key == key);
                if !editing && !matches!(part, Part::Copy | Part::Discard) {
                    self.take_block(row, tag, part, window, cx);
                    return;
                }
                if !editing {
                    match part {
                        Part::Copy => self.draft_copy(key, window, cx),
                        _ => self.draft_discard(key, cx),
                    }
                    return;
                }
                self.edit_part(part, window, cx);
            }
            None => {
                self.take_block(row, down.at.tag, part, window, cx);
            }
        }
        cx.notify();
    }

    /// Block `s` (row `id`, not held) with its editable parts made targets
    /// ([`Part`]): a click there takes it, then acts (module docs).
    pub(crate) fn editable_part(&self, el: Stateful<Div>, id: u64, tag: Tag, part: Part, cx: &Context<Self>) -> Stateful<Div> {
        let pane = self.panes.cur.id;
        let pr = crate::lease::Press { at: crate::lease::Target { row: id, tag }, act: crate::lease::Act::Edit(part), group: None };
        el.on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                this.block_down(pane, pr, cx);
            }),
        )
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseUpEvent, window, cx| {
                cx.stop_propagation();
                this.in_pane(pane, |this| this.draft_up(id, part, window, cx));
            }),
        )
    }

    /// The drafts dock, over the composer (module docs): the session's
    /// drafts with no row drawn in this pane (gone, compacted, or another
    /// draft of the block has its row: [`crate::drafts::Drafts::rowed`]),
    /// a line each, with Copy to composer and Discard.
    pub(crate) fn drafts_dock(&self, p: &Palette, m: &Metrics, cx: &mut Context<Self>) -> Option<AnyElement> {
        let session = self.edit_session()?;
        let rowless: Vec<&Draft> = self
            .block_drafts
            .of(&session)
            .filter(|d| self.block_drawn(crate::blocks::row_id(&d.id)).is_none() || !self.block_drafts.rowed(&session, d))
            .collect();
        if rowless.is_empty() {
            return None;
        }
        let pane = self.panes.cur.id;
        let mut col = v_flex()
            .w_full()
            .px(px(12.))
            .py(px(4.))
            .gap(px(2.))
            .bg(p.chrome)
            .border_t_1()
            .border_color(p.frame)
            .debug_selector(|| "drafts-dock".into());
        for d in rowless {
            let dk = d.key;
            let why = d.ended.and_then(Fate::says).unwrap_or_else(|| "held · its block isn't drawn here".into());
            let title = d.base_block.title.clone().unwrap_or_else(|| d.id.to_string());
            let t = format!("{title} · {why}");
            let tt = t.clone();
            let act = |label: &'static str, copy: bool| {
                let of = title.clone();
                div()
                    .id((label, dk))
                    .flex_none()
                    .px(px(8.))
                    .cursor_pointer()
                    .font_family(fonts::SANS)
                    .text_px(m.meta)
                    .text_color(p.prose_t)
                    .bg(p.well_d)
                    .debug_selector(move || format!("dock-button {label} {of}"))
                    .child(label)
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.in_pane(pane, |this| {
                            if copy {
                                this.draft_copy(dk, window, cx);
                            } else {
                                this.draft_discard(dk, cx);
                            }
                        });
                    }))
            };
            col = col.child(
                h_flex()
                    .w_full()
                    .gap(px(8.))
                    .items_center()
                    .whitespace_nowrap()
                    .debug_selector(move || format!("dock-draft {tt}"))
                    .child(crate::app::display("DRAFT", fonts::size::D_S, p.focus).flex_none())
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .overflow_hidden()
                            .text_ellipsis()
                            .font_family(fonts::SANS)
                            .text_px(m.meta)
                            .text_color(p.dim_t)
                            .child(t),
                    )
                    .child(act("Copy to composer", true))
                    .child(act("Discard", false)),
            );
        }
        Some(col.into_any_element())
    }
}

/// The words of the draft that ended (none: none ended).
fn d_ended_words(d: &Option<&Draft>) -> String {
    d.and_then(|d| d.ended).and_then(Fate::says).unwrap_or_default()
}
