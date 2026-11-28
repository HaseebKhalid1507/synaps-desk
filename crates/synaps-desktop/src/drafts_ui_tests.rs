//! D8a's window tests (synaps-cards `docs/blocks.md` v1.3 §3.2, §3.7's who
//! may edit, §5.2, §1.8's drafts row; §8.2's D8 row, its first half):
//! take, edit and hold a block through the window's own keys and pointer,
//! as the hand has them.
//!
//! The list: drafts over the bound board; the plan, note and
//! proposed-diff editors; the pre-check; stale drafts and orphans with
//! Copy to composer; read-only without input authority; IME in label
//! fields; 8 drafts across successive compactions; the
//! never-resolves-a-prompt tests, click and key.

use cards_spec::block::Ref;
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, EntityInputHandler as _, TestAppContext};
use serde_json::{Value, json};

use super::answer_tests::{attachment, has, heard};
use super::blocks_ui_tests::{bounds, click, drawn, open_session, round, turn};
use super::keys_tests::{composer_text, keys_at, press, prompt, prompt_pending};
use super::{open_at, settle, wait};
use crate::app::AppView;
use crate::draft_view::{ESC_AGAIN, KEEPS_WORKING};
use crate::drafts::{FULL, Fate, NOTHING_LEFT};

type W = (AnyWindowHandle, Entity<AppView>);

pub(super) fn row(name: &str) -> u64 {
    crate::blocks::row_id(&Ref::parse(name).expect("an id"))
}

/// The blocks demo, a live turn, then `ops` put at the transcript's end
/// (the last is the newest block in view), the list at its end.
fn with(cx: &mut TestAppContext, ops: Value) -> W {
    let (win, view, _events) = open_at(cx, (1500., 1000.), crate::theme::Panels::default().text_size);
    open_session(cx, win, &view, crate::demo::BLOCKS, crate::tabs::Place::Current);
    super::keys_tests::at_rest(cx, win, &view);
    turn(cx, win, &view);
    round(cx, win, &view, &[("toolu_d0", ops, true)]);
    tail(cx, win, &view);
    (win, view)
}

/// A plan `p` (`fix`, `test`, `deploy`, todo), and a note and a proposal.
fn planned(cx: &mut TestAppContext) -> W {
    with(
        cx,
        json!([
            {"op":"put","id":"n","kind":"note","title":"Root cause","body":{"text":"It is the retry loop"}},
            {"op":"put","id":"d","kind":"diff","title":"Fix: serialize","body":{"file":"src/a.rs","state":"proposed","hunks":[
                {"id":"h1","lines":["+a"],"state":"open"},{"id":"h2","lines":["-b"],"state":"open"}]}},
            {"op":"put","id":"p","kind":"plan","title":"Close the race","body":{"steps":[
                {"id":"fix","label":"Fix","state":"todo"},{"id":"test","label":"Test","state":"todo"},{"id":"deploy","label":"Deploy","state":"todo"}]}}
        ]),
    )
}

fn tail(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    view.update(cx, |v, cx| {
        v.panes.cur.list.scroll_to_end();
        cx.notify();
    });
    cx.run_until_parked();
    wait(cx, w, view, 1000);
}

/// One agent round of `ops` on the live turn, bound.
fn ops(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, tool: &str, ops: Value) {
    round(cx, w, view, &[(tool, ops, true)]);
    wait(cx, w, view, 300);
}

/// Ctrl+G, `k` `times`, then `e` on the walked block: its ring painted
/// first, as a hand would wait for it.
pub(super) fn walk_take(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, ups: usize) {
    press(cx, w, "ctrl-g");
    wait(cx, w, view, 300);
    for _ in 0..ups {
        press(cx, w, "k");
        wait(cx, w, view, 200);
    }
    press(cx, w, "e");
    wait(cx, w, view, 300);
}

/// The draft of block `name` on the session on screen, if it has one: its
/// view's body, and what became of it.
fn draft(cx: &mut TestAppContext, view: &Entity<AppView>, name: &str) -> Option<(synaps_client::blocks::Body, Option<Fate>, bool)> {
    let id = Ref::parse(name).expect("an id");
    view.read_with(cx, |v, _| {
        let s = v.model.lives[v.panes.cur.live].active.as_ref()?.session_id.clone();
        let d = v.block_drafts.of(&s).filter(|d| d.id == id).last()?;
        Some((d.view().body, d.ended, d.changed()))
    })
}

fn drafts(cx: &mut TestAppContext, view: &Entity<AppView>) -> usize {
    view.read_with(cx, |v, _| v.block_drafts.len())
}

/// Step `step` of the plan `p`'s draft: its state, stop and label.
fn step(cx: &mut TestAppContext, view: &Entity<AppView>, step: &str) -> (String, bool, String) {
    let Some((synaps_client::blocks::Body::Plan(p), _, _)) = draft(cx, view, "p") else { panic!("a plan's draft") };
    let s = p.steps.iter().find(|s| s.id.as_str() == step).expect("the step");
    (s.state.as_str().to_string(), s.stop, s.label.clone())
}

fn order(cx: &mut TestAppContext, view: &Entity<AppView>) -> Vec<String> {
    let Some((synaps_client::blocks::Body::Plan(p), _, _)) = draft(cx, view, "p") else { panic!("a plan's draft") };
    p.steps.iter().map(|s| s.id.to_string()).collect()
}

pub(super) fn footer(cx: &mut TestAppContext, w: AnyWindowHandle, name: &str, text: &str) -> bool {
    has(cx, w, format!("draft-footer {} {text}", row(name)))
}

/// The draft keys are the pane's: neither the composer's nor the window's.
fn editing(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) -> bool {
    cx.update_window(w, |_, window, cx| view.read(cx).panes.cur.edit.holds(window)).expect("the window")
}

/// The pane's open draft field, a one-line one.
fn field(cx: &mut TestAppContext, view: &Entity<AppView>) -> Option<Entity<gpui_kit::component::input::InputState>> {
    view.read_with(cx, |v, _| v.panes.cur.edit.inputs().0.cloned())
}

fn says(cx: &mut TestAppContext, view: &Entity<AppView>) -> Option<String> {
    view.read_with(cx, |v, _| v.panes.cur.edit.note.as_ref().map(|n| n.1.clone()))
}

/// The bound board the session's board keeps for the pre-check, if it does.
fn kept(cx: &mut TestAppContext, view: &Entity<AppView>) -> bool {
    view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].transcript.drawn().is_some_and(|d| d.board.is_some()))
}

/// The session's compaction, as the daemon says it.
pub(super) fn compact(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    let sid = attachment(cx, view).1;
    let j = view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].transcript.journal().to_string());
    heard(
        cx,
        w,
        view,
        vec![json!({"ev": "compaction_applied", "previous_session_id": sid, "session_id": j, "chains_advanced": [], "msg_count": 4})],
    );
    wait(cx, w, view, 300);
}

// ── taking, and the plan's editor ─────────────────────────────────────

/// `e` in the walk takes the walked plan: it draws as its draft, headed
/// `EDITING · r1`, the pane holding the keys; `x`, `b`, `j`, Shift+J, `r`
/// and `a` edit the draft, never the block. The board keeps the bound
/// board for the pre-check while a draft is held.
#[gpui_kit::test]
fn e_takes_a_plan_and_its_keys_edit_the_draft(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    let p = row("p");
    assert!(!kept(cx, &view), "no draft: no bound board kept");
    walk_take(cx, w, &view, 0);
    assert!(has(cx, w, format!("block-draft {p}")), "drawn as its draft");
    assert!(has(cx, w, format!("draft-head {p} EDITING · r1")));
    assert!(footer(cx, w, "p", KEEPS_WORKING), "a hold doesn't pause the agent");
    assert!(editing(cx, w, &view), "the draft has the keys");
    assert!(kept(cx, &view), "the bound board kept for the pre-check");
    press(cx, w, "x");
    assert_eq!(step(cx, &view, "fix").0, "skipped", "x strikes the step under the cursor");
    press(cx, w, "j");
    press(cx, w, "b");
    assert!(step(cx, &view, "test").1, "b stops before it");
    press(cx, w, "shift-j");
    assert_eq!(order(cx, &view), ["fix", "deploy", "test"], "Shift+J moves it down");
    press(cx, w, "shift-k");
    assert_eq!(order(cx, &view), ["fix", "test", "deploy"], "Shift+K up");
    press(cx, w, "r");
    let f = field(cx, &view).expect("r opens the label's field");
    cx.update_window(w, |_, window, cx| f.update(cx, |s, cx| s.replace_text_in_range(Some(0..4), "Test twice", window, cx)))
        .expect("the window");
    settle(cx, w, 8);
    assert_eq!(step(cx, &view, "test").2, "Test twice", "typed into the draft");
    press(cx, w, "enter");
    assert!(field(cx, &view).is_none() && editing(cx, w, &view), "Enter leaves the field to the block's keys");
    press(cx, w, "a");
    let f = field(cx, &view).expect("a: the added step's label");
    cx.update_window(w, |_, window, cx| f.update(cx, |s, cx| s.replace_text_in_range(None, "Check logs", window, cx))).expect("the window");
    press(cx, w, "enter");
    let o = order(cx, &view);
    assert_eq!(o.len(), 4);
    assert!(o[2].starts_with("u-"), "added after the cursor, a u- id: {o:?}");
    // The block itself is as the agent left it.
    let fix = view.read_with(cx, |v, _| {
        let (_, s) = v.block_shown(row("p")).expect("drawn");
        match &s.block.body {
            synaps_client::blocks::Body::Plan(pl) => pl.steps[0].state,
            _ => panic!("a plan"),
        }
    });
    assert_eq!(fix, cards_spec::block::StepState::Todo, "nothing sent, nothing changed on the board");
}

/// The draft draws over `B` (§3.4): an agent tick while held is counted in
/// the header, never drawn under the user; struck, then run past, the
/// pre-check lists it and leaves nothing to hand back: Hand back is off,
/// and Enter sends nothing.
#[gpui_kit::test]
fn agent_changes_are_counted_and_the_pre_check_lists_what_fails(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    let p = row("p");
    walk_take(cx, w, &view, 0);
    press(cx, w, "j");
    press(cx, w, "j");
    press(cx, w, "x");
    assert_eq!(step(cx, &view, "deploy").0, "skipped");
    assert!(has(cx, w, format!("draft-button {p} Hand back on ready")), "something to hand back");
    ops(cx, w, &view, "toolu_t1", json!([{"op":"patch","id":"p","set":{"steps.deploy.state":"doing"}}]));
    assert!(has(cx, w, format!("draft-head {p} EDITING · r1 · 1 agent change")), "counted");
    assert_eq!(step(cx, &view, "deploy").0, "skipped", "drawn over B, not under the user");
    assert!(footer(cx, w, "p", "1 change no longer applies: deploy already ran"));
    assert!(footer(cx, w, "p", NOTHING_LEFT));
    assert!(has(cx, w, format!("draft-button {p} Hand back off")), "nothing left: not ready");
    ops(cx, w, &view, "toolu_t2", json!([{"op":"patch","id":"p","set":{"steps.fix.label":"Fix it"}}]));
    assert!(has(cx, w, format!("draft-head {p} EDITING · r1 · 2 agent changes")));
    press(cx, w, "enter");
    assert_eq!(says(cx, &view).as_deref(), Some(NOTHING_LEFT), "nothing left: nothing goes");
    view.read_with(cx, |v, _| assert!(v.sent.is_none() && v.edit_sends.is_empty(), "nothing sent"));
}

/// Cancel (§3.2): with no changes Esc lets the draft go, the keys to the
/// composer; with changes the first Esc says `Esc again discards`, the
/// second discards. Another printable key leaves, typed, the draft held.
#[gpui_kit::test]
fn esc_lets_an_unchanged_draft_go_and_asks_once_for_a_changed_one(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    walk_take(cx, w, &view, 0);
    press(cx, w, "escape");
    assert_eq!(drafts(cx, &view), 0, "no changes: gone");
    assert_eq!(keys_at(cx, w, &view), (true, false), "the composer, never the window");
    walk_take(cx, w, &view, 0);
    press(cx, w, "x");
    press(cx, w, "escape");
    assert!(footer(cx, w, "p", ESC_AGAIN));
    assert_eq!(drafts(cx, &view), 1);
    press(cx, w, "escape");
    assert_eq!(drafts(cx, &view), 0, "the second Esc discards");
    assert_eq!(keys_at(cx, w, &view), (true, false));
    walk_take(cx, w, &view, 0);
    press(cx, w, "x");
    press(cx, w, "q");
    assert_eq!(composer_text(cx, &view), "q", "typed into the composer");
    assert_eq!(drafts(cx, &view), 1, "the draft stays held");
    assert!(has(cx, w, format!("block-draft {}", row("p"))));
}

/// A click on an editable part takes the block, then acts: a step's label
/// opens its field. An IME composition owns its keys: Esc and Enter
/// mid-composition leave the field open; committed, Enter leaves it.
#[gpui_kit::test]
fn a_click_on_a_label_takes_it_and_an_ime_owns_the_fields_keys(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    let p = row("p");
    let at = drawn(cx, w, format!("block-label {p} test")).center();
    click(cx, w, at);
    wait(cx, w, &view, 300);
    assert!(has(cx, w, format!("block-draft {p}")), "the click took it");
    let f = field(cx, &view).expect("and opened the label's field");
    assert_eq!(view.read_with(cx, |_, cx| f.read(cx).value().to_string()), "Test");
    let mark = |cx: &mut TestAppContext, s: &str| {
        cx.update_window(w, |_, window, cx| f.update(cx, |st, cx| st.replace_and_mark_text_in_range(None, s, None, window, cx)))
            .expect("the window");
        settle(cx, w, 8);
        let composing = cx.update_window(w, |_, window, cx| view.update(cx, |v, cx| v.composing_now(window, cx))).expect("the window");
        assert!(composing, "composing {s}");
    };
    mark(cx, "か");
    assert_eq!(step(cx, &view, "test").2, "Test", "marked text isn't the draft's yet");
    press(cx, w, "enter");
    assert!(field(cx, &view).is_some(), "the Enter that met a composition is the IME's");
    mark(cx, "か");
    press(cx, w, "escape");
    assert!(field(cx, &view).is_some(), "the Esc that ends a composition only ends it");
    let left = view.read_with(cx, |_, cx| f.read(cx).value().to_string());
    assert!(left.ends_with('か'), "{left}");
    assert_eq!(step(cx, &view, "test").2, left, "what it left is the draft's");
    mark(cx, "な");
    cx.update_window(w, |_, window, cx| f.update(cx, |st, cx| st.replace_text_in_range(None, "な", window, cx))).expect("the window");
    settle(cx, w, 8);
    assert_eq!(step(cx, &view, "test").2, format!("{left}な"), "committed by the IME");
    press(cx, w, "enter");
    assert!(field(cx, &view).is_none(), "no composition: Enter leaves the field");
    assert!(editing(cx, w, &view));
    // A strike gutter's click strikes.
    let test = crate::draft_view::item_hash(&cards_spec::block::ItemId::parse("test").expect("an id"));
    let at = drawn(cx, w, format!("draft-strike-gutter {p} {test}")).center();
    click(cx, w, at);
    assert_eq!(step(cx, &view, "test").0, "skipped");
}

// ── the note and the proposal ─────────────────────────────────────────

/// A note's text (`r`, a text area) and title (`t`); a proposal's hunks
/// kept (`y`) and dropped (`n`), by key and by click.
#[gpui_kit::test]
fn a_notes_text_and_title_and_a_proposals_hunks_are_edited(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    walk_take(cx, w, &view, 2);
    assert!(has(cx, w, format!("block-draft {}", row("n"))), "the note taken");
    press(cx, w, "t");
    let f = field(cx, &view).expect("its title's field");
    cx.update_window(w, |_, window, cx| f.update(cx, |s, cx| s.replace_text_in_range(Some(0..10), "The cause", window, cx)))
        .expect("the window");
    settle(cx, w, 8);
    let Some((_, _, true)) = draft(cx, &view, "n") else { panic!("the note's draft, changed") };
    press(cx, w, "tab");
    assert!(field(cx, &view).is_none(), "Tab leaves the field");
    press(cx, w, "r");
    let a = view.read_with(cx, |v, _| v.panes.cur.edit.inputs().1.cloned()).expect("its text's area");
    cx.update_window(w, |_, window, cx| a.update(cx, |s, cx| s.replace_text_in_range(None, " · or the cache", window, cx)))
        .expect("the window");
    settle(cx, w, 8);
    let Some((synaps_client::blocks::Body::Note(n), _, true)) = draft(cx, &view, "n") else { panic!("the note's draft, changed") };
    assert!(n.text.contains("or the cache"), "{}", n.text);
    press(cx, w, "escape");
    press(cx, w, "escape");
    press(cx, w, "escape");
    assert_eq!(drafts(cx, &view), 0, "the field, then the note's draft, discarded");
    walk_take(cx, w, &view, 1);
    let d = row("d");
    assert!(has(cx, w, format!("block-draft {d}")), "the proposal taken");
    press(cx, w, "y");
    press(cx, w, "j");
    press(cx, w, "n");
    assert!(has(cx, w, format!("draft-hunk {d} h1 kept")) && has(cx, w, format!("draft-hunk {d} h2 dropped at")));
    let h1 = crate::draft_view::item_hash(&cards_spec::block::ItemId::parse("h1").expect("an id"));
    let at = drawn(cx, w, format!("draft-drop {d} {h1}")).center();
    click(cx, w, at);
    assert!(has(cx, w, format!("draft-hunk {d} h1 dropped at")), "a click drops it, the cursor on it");
}

// ── stale, orphaned ───────────────────────────────────────────────────

/// The agent replaces the held plan: the draft goes stale in its row
/// (`the agent rewrote this block (r1)`), Hand back gone, and it takes no
/// edit. Walked to again, `e` takes its keys back; Copy to composer puts
/// line 1's words there and lets the draft go.
#[gpui_kit::test]
fn a_replaced_block_goes_stale_and_copy_to_composer_takes_its_words(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    let p = row("p");
    walk_take(cx, w, &view, 0);
    press(cx, w, "x");
    ops(
        cx,
        w,
        &view,
        "toolu_rp",
        json!([{"op":"replace","id":"p","kind":"plan","body":{"steps":[{"id":"fix","label":"Another","state":"todo"}]}}]),
    );
    assert!(has(cx, w, format!("draft-head {p} the agent rewrote this block (r1)")), "stale, in its row");
    assert!(!has(cx, w, format!("draft-button {p} Hand back off")) && has(cx, w, format!("draft-button {p} Copy to composer on")));
    press(cx, w, "x");
    assert_eq!(says(cx, &view).as_deref(), Some("the agent rewrote this block (r1)"), "a stale draft takes no edit");
    press(cx, w, "ctrl-g");
    assert!(!editing(cx, w, &view), "the walk has the keys");
    walk_take(cx, w, &view, 0);
    assert!(editing(cx, w, &view), "e takes the stale draft's keys again");
    press(cx, w, "b");
    assert_eq!(says(cx, &view).as_deref(), Some("the agent rewrote this block (r1)"), "and says why on any key but c and Esc");
    wait(cx, w, &view, 1000);
    let at = drawn(cx, w, format!("draft-button {p} Copy to composer on")).center();
    click(cx, w, at);
    assert_eq!(composer_text(cx, &view), "Close the race → skip fix");
    assert_eq!(drafts(cx, &view), 0, "copied: let go");
    assert!(!has(cx, w, format!("block-draft {p}")), "the new plan draws");
}

/// A compaction while held: each draft is an orphan, in place, drawn in
/// the drafts dock (`compacted · this draft has no block`), with Copy to
/// composer (line 1's words) and Discard.
#[gpui_kit::test]
fn a_compaction_during_a_hold_makes_orphans_in_the_dock(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    walk_take(cx, w, &view, 0);
    press(cx, w, "x");
    walk_take(cx, w, &view, 2);
    assert_eq!(drafts(cx, &view), 2, "the plan's and the note's");
    compact(cx, w, &view);
    assert_eq!(draft(cx, &view, "p").map(|d| d.1), Some(Some(Fate::Orphan)), "an orphan, in place");
    assert_eq!(draft(cx, &view, "n").map(|d| d.1), Some(Some(Fate::Orphan)));
    assert!(has(cx, w, "drafts-dock".into()));
    assert!(has(cx, w, "dock-draft Close the race · compacted · this draft has no block".into()));
    assert!(has(cx, w, "dock-draft Root cause · compacted · this draft has no block".into()));
    assert!(!has(cx, w, format!("block-draft {}", row("p"))), "no longer in the block's row");
    assert!(!editing(cx, w, &view), "an orphan's keys are the dock's buttons");
    let at = drawn(cx, w, "dock-button Copy to composer Close the race".into()).center();
    click(cx, w, at);
    assert_eq!(composer_text(cx, &view), "Close the race → skip fix");
    assert_eq!(drafts(cx, &view), 1);
    let at = drawn(cx, w, "dock-button Discard Root cause".into()).center();
    click(cx, w, at);
    assert_eq!(drafts(cx, &view), 0);
    assert!(!has(cx, w, "drafts-dock".into()));
}

/// 8 drafts per session across successive compactions (§1.8): none
/// dropped, never more than 8, and the 9th take refused, saying so.
#[gpui_kit::test]
fn eight_drafts_across_successive_compactions_and_a_ninth_is_refused(cx: &mut TestAppContext) {
    let notes: Vec<Value> =
        (0..9).map(|k| json!({"op":"put","id":format!("n{k}"),"kind":"note","body":{"text":format!("note {k}")}})).collect();
    let (w, view) = with(cx, Value::Array(notes));
    let take = |cx: &mut TestAppContext, k: usize| {
        let r = row(&format!("n{k}"));
        let tag = view.read_with(cx, |v, _| v.block_shown(r).expect("drawn").1.block.tag);
        cx.update_window(w, |_, window, cx| view.update(cx, |v, cx| v.take_block(r, tag, crate::draft_view::Part::Take, window, cx)))
            .expect("the window")
    };
    for k in 0..4 {
        assert!(take(cx, k), "room for {k}");
    }
    compact(cx, w, &view);
    for k in 4..8 {
        assert!(take(cx, k), "room for {k}");
    }
    compact(cx, w, &view);
    assert_eq!(drafts(cx, &view), 8, "none dropped");
    assert!(!take(cx, 8), "a 9th");
    assert_eq!(says(cx, &view).as_deref(), Some(FULL));
    assert!(has(cx, w, format!("block-says {} {FULL}", row("n8"))), "the block says why");
    compact(cx, w, &view);
    assert_eq!(drafts(cx, &view), 8, "a third compaction: still 8");
}

// ── who may edit ──────────────────────────────────────────────────────

/// Without the session's input authority the block takes nothing and
/// says why: whose the input is and how to take it, or the gate's reason.
#[gpui_kit::test]
fn without_input_authority_nothing_is_taken_and_the_block_says_why(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    let owner = view.update(cx, |v, cx| {
        let a = v.model.lives[v.panes.cur.live].active.as_mut().unwrap();
        a.input = synaps_client::ClientInput::Exclusive;
        a.owner = Some(a.client + 1);
        cx.notify();
        a.client + 1
    });
    settle(cx, w, 8);
    walk_take(cx, w, &view, 0);
    assert_eq!(drafts(cx, &view), 0, "read-only");
    let words = format!("edit from client #{owner} · /takeover");
    assert_eq!(says(cx, &view).as_deref(), Some(words.as_str()));
    assert!(has(cx, w, format!("block-says {} {words}", row("p"))));
    press(cx, w, "escape");
    let at = drawn(cx, w, format!("block-label {} test", row("p"))).center();
    click(cx, w, at);
    assert_eq!(drafts(cx, &view), 0, "a click takes nothing either");
    view.update(cx, |v, cx| {
        let a = v.model.lives[v.panes.cur.live].active.as_mut().unwrap();
        a.input = synaps_client::ClientInput::Shared;
        a.mode = synaps_client::AttachMode::Observe;
        cx.notify();
    });
    settle(cx, w, 8);
    let at = drawn(cx, w, format!("block-label {} test", row("p"))).center();
    click(cx, w, at);
    assert_eq!(says(cx, &view).as_deref(), Some("read-only: this window observes the session"));
    assert_eq!(drafts(cx, &view), 0);
}

// ── never a prompt's answer (design §5.1) ─────────────────────────────

/// With a prompt up, the draft keys are the pane's: taking by key,
/// editing, Enter and Esc twice never answer or deny it, and the keys go
/// back to the composer, never to the window.
#[gpui_kit::test]
fn the_draft_keys_never_resolve_a_prompt(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    prompt(cx, w, &view);
    walk_take(cx, w, &view, 0);
    assert!(editing(cx, w, &view), "taken with a prompt up");
    prompt_pending(cx, &view, "e");
    for k in ["x", "j", "b", "y", "n"] {
        press(cx, w, k);
        prompt_pending(cx, &view, k);
    }
    press(cx, w, "escape");
    prompt_pending(cx, &view, "one Esc");
    // Enter hands it back (D8b): a message of its own, never the prompt's
    // answer.
    press(cx, w, "enter");
    prompt_pending(cx, &view, "Enter, handing back");
    view.read_with(cx, |v, _| assert_eq!(v.edit_sends.len(), 1, "handed back"));
    assert_eq!(keys_at(cx, w, &view), (true, false), "the composer, not the window");
    press(cx, w, "escape");
    prompt_pending(cx, &view, "an Esc after");
}

/// With a prompt up, a click that takes a block and its field's Enter,
/// Esc and Tab never answer or deny it.
#[gpui_kit::test]
fn a_click_on_a_draft_never_resolves_a_prompt(cx: &mut TestAppContext) {
    let (w, view) = planned(cx);
    prompt(cx, w, &view);
    let at = drawn(cx, w, format!("block-label {} test", row("p"))).center();
    click(cx, w, at);
    wait(cx, w, &view, 300);
    assert!(field(cx, &view).is_some(), "taken, its field open");
    prompt_pending(cx, &view, "the click");
    for k in ["y", "n", "enter"] {
        press(cx, w, k);
        prompt_pending(cx, &view, k);
    }
    press(cx, w, "r");
    press(cx, w, "tab");
    press(cx, w, "escape");
    prompt_pending(cx, &view, "Esc on the draft");
    let at = drawn(cx, w, format!("draft-button {} Discard on", row("p"))).center();
    click(cx, w, at);
    prompt_pending(cx, &view, "Discard");
    assert_eq!(drafts(cx, &view), 0);
    assert!(bounds(cx, w, format!("block-draft {}", row("p"))).is_none());
}
