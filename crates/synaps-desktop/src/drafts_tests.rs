//! The draft model's tests (synaps-cards `docs/blocks.md` v1.3 §3.2,
//! §3.3, §3.4, §1.8's drafts row, §5.2; §8.2's D8 row, D8a's half): the
//! editors, the patch and line 1 in the grammar's own words, B2's
//! pre-check on §3.4's traces, stale and orphaned drafts, the caps.

use std::sync::Arc;

use cards_spec::block::{Generation, ItemId, Ref};
use cards_spec::board::{Board, Code, Key, State};
use serde_json::{Value, json};

use super::*;

/// A history of block calls after the first message `first` (its
/// generation's), each round one call whose receipt binds it.
fn history(first: &str, rounds: &[Value]) -> Vec<Value> {
    let mut m = vec![json!({"role": "user", "content": first})];
    for (k, ops) in rounds.iter().enumerate() {
        let id = format!("toolu_{k}");
        m.push(json!({"role": "assistant", "content": [{"type": "tool_use", "id": id, "name": "cards_block", "input": {"ops": ops}}]}));
        m.push(json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": id,
            "content": synaps_client::synthetic::block_receipt(ops)}]}));
    }
    m
}

/// The bound board after `rounds`, and its generation.
fn board(rounds: &[Value]) -> (State, Generation) {
    board_after("go", rounds)
}

fn board_after(first: &str, rounds: &[Value]) -> (State, Generation) {
    let (b, events) = Board::of_history(&history(first, rounds));
    (b.bound(&events), b.generation())
}

fn id(s: &str) -> ItemId {
    ItemId::parse(s).expect("an item id")
}

fn block(st: &State, name: &str) -> Arc<cards_spec::board::Block> {
    Arc::new(st.block(&Ref::parse(name).expect("an id")).expect("live").clone())
}

/// A draft of block `name` on `st`.
fn take(st: &State, g: Generation, name: &str) -> Draft {
    Draft::of(block(st, name), g).expect("editable")
}

/// §3.3's plan, at r1: `fix`, `test`, `bandaid`, `deploy`, all todo.
fn plan() -> Value {
    json!([{"op":"put","id":"plan","kind":"plan","title":"Close the refresh race","body":{"steps":[
        {"id":"fix","label":"Fix the race","state":"todo"},
        {"id":"test","label":"Test it","state":"todo"},
        {"id":"bandaid","label":"Remove the bandaid","state":"todo"},
        {"id":"deploy","label":"Deploy","state":"todo"}]}}])
}

fn tick(step: &str, state: &str) -> Value {
    json!([{"op":"patch","id":"plan","set":{format!("steps.{step}.state"): state}}])
}

/// What line 2 of `text` is.
fn line2(text: &str) -> &str {
    text.split('\n').nth(1).expect("line 2")
}

// ── the patch and line 1, in the grammar's words (§3.3) ───────────────

/// §3.3's first example, from a plan whose `deploy` the user moves before
/// `bandaid`, then strikes `bandaid` and stops before `deploy`: line 1 and
/// line 2 as the grammar writes them, `of` the incarnation's tag.
#[test]
fn a_plan_edit_is_written_as_the_grammars_first_example() {
    let (st, g) = board(&[plan()]);
    let mut d = take(&st, g, "plan");
    assert_eq!(d.base, 1);
    assert!(d.shift(&id("deploy"), -1), "deploy up, before bandaid");
    d.strike(&id("bandaid")).expect("a todo step strikes");
    d.stop(&id("deploy")).expect("a todo step takes a stop");
    let c = d.check(&st);
    assert!(c.fails.is_empty() && c.off.is_none(), "{c:?}");
    let text = c.message.expect("a message");
    assert_eq!(
        text,
        format!(
            "[block plan edited r1] Close the refresh race → order fix, test, deploy, bandaid · skip bandaid · stop before deploy\n\
             {{\"of\":\"{}\",\"order\":{{\"steps\":[\"fix\",\"test\",\"deploy\",\"bandaid\"]}},\"set\":{{\"steps.bandaid.state\":\"skipped\",\"steps.deploy.stop\":true}}}}",
            d.tag
        )
    );
    // It reads back as the edit it is (the board admits it).
    assert!(cards_spec::block::message::parse(&text).is_some(), "the grammar reads it back");
}

/// §3.3's second: a step added after `test` is an append of its `u-` id,
/// `todo` with no stop, and the order that places it; line 1 says where.
#[test]
fn an_added_step_is_an_append_and_the_order_that_places_it() {
    let (st, g) = board(&[json!([{"op":"put","id":"plan","kind":"plan","title":"Close the refresh race","body":{"steps":[
        {"id":"fix","label":"Fix","state":"done"},{"id":"test","label":"Test","state":"todo"},{"id":"deploy","label":"Deploy","state":"todo"}]}}])]);
    let mut d = take(&st, g, "plan");
    let u = ItemId::mint(0x0A3B_C5D7);
    assert!(u.is_minted());
    d.add(Some(&id("test")), u.clone(), "Check staging logs").expect("room");
    let text = d.check(&st).message.expect("a message");
    assert_eq!(
        text,
        format!(
            "[block plan edited r1] Close the refresh race → add “Check staging logs” after test\n\
             {{\"of\":\"{}\",\"append\":{{\"steps\":[{{\"id\":\"{u}\",\"label\":\"Check staging logs\",\"state\":\"todo\"}}]}},\"order\":{{\"steps\":[\"fix\",\"test\",\"{u}\",\"deploy\"]}}}}",
            d.tag,
            u = u.as_str()
        )
    );
    // Added last, an append alone puts it there: no order.
    let mut d = take(&st, g, "plan");
    d.add(Some(&id("deploy")), u.clone(), "Last").expect("room");
    assert!(!line2(&d.check(&st).message.expect("a message")).contains("order"), "no order when the append places it");
}

/// §3.3's fourth: a proposal's hunks kept and dropped; a note's text; and
/// a change taken back is no intent.
#[test]
fn hunks_and_a_notes_text_are_sets_and_a_change_taken_back_is_none() {
    let (st, g) = board(&[json!([
        {"op":"put","id":"auth-diff","kind":"diff","title":"Fix: serialize rotation","body":{"file":"src/a.rs","state":"proposed","hunks":[
            {"id":"h1","lines":["+a"],"state":"open"},{"id":"h2","lines":["+b"],"state":"open"},{"id":"h3","lines":["-c"],"state":"open"}]}},
        {"op":"put","id":"cause","kind":"note","title":"Root cause","body":{"text":"It is the retry loop"}}])]);
    let mut d = take(&st, g, "auth-diff");
    d.hunk(&id("h1"), true).expect("a proposal");
    d.hunk(&id("h2"), true).expect("a proposal");
    d.hunk(&id("h3"), false).expect("a proposal");
    assert_eq!(
        d.check(&st).message.expect("a message"),
        format!(
            "[block auth-diff edited r1] Fix: serialize rotation → keep h1, h2 · drop h3\n\
             {{\"of\":\"{}\",\"set\":{{\"hunks.h1.state\":\"kept\",\"hunks.h2.state\":\"kept\",\"hunks.h3.state\":\"dropped\"}}}}",
            d.tag
        )
    );
    d.hunk(&id("h3"), false).expect("the same again");
    assert!(!d.check(&st).message.expect("a message").contains("h3"), "a drop taken back");
    let mut n = take(&st, g, "cause");
    n.note(false, "First guess: the retry loop").expect("within");
    assert_eq!(
        n.check(&st).message.expect("a message"),
        format!(
            "[block cause edited r1] Root cause → text “First guess: the retry loop”\n{{\"of\":\"{}\",\"set\":{{\"text\":\"First guess: the retry loop\"}}}}",
            n.tag
        )
    );
    n.note(false, "It is the retry loop").expect("within");
    assert!(!n.changed(), "back to B's text: no intent");
    assert_eq!(n.check(&st).off.as_deref(), Some("no changes yet"));
}

/// Text and order are the user's, execution truth the agent's (§3.2): a
/// step that ran is renamed and moved, never struck nor stopped; an added
/// step takes no stop, and struck it is taken out; a diff that isn't a
/// proposal keeps no hunk.
#[test]
fn a_step_that_ran_is_renamed_and_moved_never_struck_or_stopped() {
    let (st, g) = board(&[json!([{"op":"put","id":"plan","kind":"plan","body":{"steps":[
        {"id":"fix","label":"Fix","state":"done"},{"id":"test","label":"Test","state":"doing"},{"id":"go","label":"Go","state":"skipped"}]}},
        {"op":"put","id":"d","kind":"diff","body":{"file":"a","state":"written","hunks":[{"id":"h1","lines":["+a"],"state":"open"}]}}])]);
    let mut d = take(&st, g, "plan");
    assert_eq!(d.strike(&id("fix")), Err(RAN));
    assert_eq!(d.stop(&id("test")), Err(RAN));
    d.rename(&id("fix"), "Fixed it").expect("a label is the user's");
    assert!(d.shift(&id("fix"), 1));
    d.strike(&id("go")).expect("a skipped step unstrikes");
    d.stop(&id("go")).expect("a skipped step takes a stop");
    let c = d.check(&st);
    assert!(c.fails.is_empty(), "{c:?}");
    let t = c.message.expect("a message");
    assert!(
        t.contains("unskip go") && t.contains("stop before go") && t.contains("rename fix “Fixed it”") && t.contains("order test, fix, go"),
        "{t}"
    );
    let u = ItemId::mint(7);
    d.add(None, u.clone(), "First").expect("room");
    assert_eq!(d.stop(&u), Err("an added step takes no stop"));
    d.strike(&u).expect("an added step struck is taken out");
    assert!(!d.check(&st).message.expect("a message").contains(u.as_str()));
    let mut w = take(&st, g, "d");
    assert_eq!(w.hunk(&id("h1"), true), Err(NOT_PROPOSAL));
    assert!(Draft::of(block(&st, "plan"), g).is_some());
}

// ── the pre-check on §3.4's traces ────────────────────────────────────

/// `deploy` is todo at r1; the user strikes it while the agent ticks it
/// doing (r2), then done (r3): handed back after, the pre-check drops the
/// skip, lists it, and leaves nothing to hand back. The draft still draws
/// over B, and counts the agent's two changes.
#[test]
fn a_skip_the_agent_ran_past_is_dropped_and_listed() {
    let (st, g) = board(&[plan()]);
    let mut d = take(&st, g, "plan");
    d.strike(&id("deploy")).expect("todo at B");
    let (now, _) = board(&[plan(), tick("deploy", "doing"), tick("deploy", "done")]);
    let c = d.check(&now);
    assert_eq!(c.fails, vec![(Key::Set(cards_spec::block::Path::parse("steps.deploy.state").unwrap()), Code::AlreadyRan)]);
    assert_eq!(c.footer().as_deref(), Some("1 change no longer applies: deploy already ran"));
    assert_eq!((c.message, c.off.as_deref()), (None, Some(NOTHING_LEFT)));
    assert_eq!(d.agent_changes(&now), 2, "r2 and r3, the agent's");
    let Body::Plan(p) = d.view().body else { panic!("a plan") };
    assert_eq!(p.steps.iter().find(|s| s.id == id("deploy")).map(|s| s.state), Some(StepState::Skipped), "drawn over B");
    // With another change that still applies, only the skip goes.
    d.rename(&id("test"), "Test it twice").expect("within");
    let c = d.check(&now);
    let t = c.message.expect("the rename goes");
    assert!(!t.contains("skip") && !t.contains("steps.deploy") && t.contains("rename test “Test it twice”"), "{t}");
    assert_eq!(c.fails.len(), 1);
}

/// The user reorders while `test` goes doing: the order applies and
/// `test` stays doing (the agent's path the user didn't touch survives).
#[test]
fn an_order_applies_over_the_agents_tick() {
    let (st, g) = board(&[plan()]);
    let mut d = take(&st, g, "plan");
    assert!(d.shift(&id("deploy"), -1));
    let (now, _) = board(&[plan(), tick("test", "doing")]);
    let c = d.check(&now);
    assert!(c.fails.is_empty() && c.message.is_some(), "{c:?}");
    assert!(!line2(c.message.as_deref().unwrap()).contains("test.state"), "the user's patch names only what the user did");
}

/// The agent removes `steps.fix`: a rename of it is gone; then appends a
/// new `fix`: the item id was spent, so the rename still names nothing.
#[test]
fn a_rename_of_a_removed_step_is_gone() {
    let (st, g) = board(&[plan()]);
    let mut d = take(&st, g, "plan");
    d.rename(&id("fix"), "Fix the race, properly").expect("within");
    let removed = json!([{"op":"patch","id":"plan","remove":["steps.fix"]}]);
    let (now, _) = board(&[plan(), removed.clone()]);
    let c = d.check(&now);
    assert_eq!(c.footer().as_deref(), Some("1 change no longer applies: fix is gone"), "{c:?}");
    let again = json!([{"op":"patch","id":"plan","append":{"steps":[{"id":"fix","label":"New fix","state":"todo"}]}}]);
    let (now, _) = board(&[plan(), removed, again]);
    assert_eq!(d.check(&now).off.as_deref(), Some(NOTHING_LEFT));
}

/// The agent replaces `plan`, keeping item id `fix` for a new task: a new
/// tag, the draft goes stale (`the agent rewrote this block (r1)`), and
/// nothing would land on the new plan. Removed and put again: the same.
/// Removed: `this block is gone`. Another generation: an orphan.
#[test]
fn a_replaced_removed_or_compacted_block_makes_its_draft_stale_or_orphaned() {
    let (st, g) = board(&[plan()]);
    let mut d = take(&st, g, "plan");
    d.strike(&id("fix")).expect("todo");
    let drawn = |st: State, generation: Generation| synaps_client::blocks::Drawn {
        generation: Some(generation),
        board: Some(Arc::new(st)),
        ..Default::default()
    };
    let replaced =
        json!([{"op":"replace","id":"plan","kind":"plan","body":{"steps":[{"id":"fix","label":"Another task","state":"todo"}]}}]);
    let (now, _) = board(&[plan(), replaced]);
    assert_eq!(d.fate(&drawn(now.clone(), g)), Fate::Rewritten(1));
    assert_eq!(Fate::Rewritten(1).says().as_deref(), Some("the agent rewrote this block (r1)"));
    assert_eq!(d.check(&now).off.as_deref(), Some("the block was rewritten"), "nothing lands on the new plan");
    let gone = json!([{"op":"remove","id":"plan"}]);
    let (now, _) = board(&[plan(), gone.clone(), plan()]);
    assert_eq!(d.fate(&drawn(now, g)), Fate::Rewritten(1), "removed and put again: another incarnation");
    let (now, _) = board(&[plan(), gone]);
    assert_eq!(d.fate(&drawn(now, g)), Fate::Gone);
    let (now, g2) = board_after("<context-summary>", &[plan()]);
    assert_ne!(g, g2, "another first message, another generation");
    assert_eq!(d.fate(&drawn(now, g2)), Fate::Orphan);
    // An ended draft stays ended, and is never checked against a board.
    d.ended = Some(Fate::Gone);
    let (st, _) = board(&[plan()]);
    assert_eq!(d.fate(&drawn(st.clone(), g)), Fate::Gone);
    assert_eq!(d.check(&st).message, None);
}

/// Copy to composer (§3.2): line 1's words without the `[block …]`
/// prefix, of every intent, the stale ones too.
#[test]
fn copy_to_composer_is_line_ones_words_without_its_prefix() {
    let (st, g) = board(&[plan()]);
    let mut d = take(&st, g, "plan");
    d.strike(&id("bandaid")).expect("todo");
    d.stop(&id("deploy")).expect("todo");
    assert_eq!(d.words(), "Close the refresh race → skip bandaid · stop before deploy");
    d.ended = Some(Fate::Rewritten(1));
    assert_eq!(d.words(), "Close the refresh race → skip bandaid · stop before deploy", "a stale draft's words are the user's");
}

// ── bounds (§1.8) ─────────────────────────────────────────────────────

/// 8 drafts per session, holds and orphans together, across successive
/// compactions: a 9th take is refused, a compaction turns holds into
/// orphans in place and drops none, and a discard makes room.
#[test]
fn eight_drafts_per_session_across_successive_compactions() {
    let rounds: Vec<Value> =
        (0..12).map(|k| json!([{"op":"put","id":format!("n{k}"),"kind":"note","body":{"text":format!("note {k}")}}])).collect();
    let (st, g) = board(&rounds);
    let mut ds = Drafts::default();
    for k in 0..5 {
        ds.take("s", &block(&st, &format!("n{k}")), g).expect("room");
    }
    assert!(ds.compacted("s"), "holds became orphans");
    assert_eq!(ds.count("s"), 5, "none dropped");
    assert!(!ds.holds("s"));
    for k in 5..8 {
        ds.take("s", &block(&st, &format!("n{k}")), g).expect("room");
    }
    assert_eq!(ds.take("s", &block(&st, "n8"), g).err(), Some(FULL), "a 9th");
    ds.compacted("s");
    assert_eq!(ds.count("s"), 8, "a second compaction: still 8, none dropped");
    assert_eq!(ds.take("s", &block(&st, "n8"), g).err(), Some(FULL));
    assert!(ds.of("s").all(|d| d.ended == Some(Fate::Orphan)));
    // Another session has its own 8.
    ds.take("t", &block(&st, "n0"), g).expect("another session's room");
    let first = ds.of("s").next().map(|d| (d.id.clone(), d.tag)).expect("one");
    assert!(ds.discard("s", &first.0, first.1));
    ds.take("s", &block(&st, "n8"), g).expect("room again");
    // A held draft is taken again, never twice.
    assert_eq!(ds.count("s"), 8);
    ds.take("s", &block(&st, "n8"), g).expect("the same draft");
    assert_eq!(ds.count("s"), 8);
}

/// Across sessions, at most 64 drafts on the desk: the 65th take is
/// refused, and says so.
#[test]
fn the_desk_holds_64_drafts_at_most() {
    let rounds: Vec<Value> = (0..8).map(|k| json!([{"op":"put","id":format!("n{k}"),"kind":"note","body":{"text":"x"}}])).collect();
    let (st, g) = board(&rounds);
    let mut ds = Drafts::default();
    for s in 0..8 {
        for k in 0..8 {
            ds.take(&format!("s{s}"), &block(&st, &format!("n{k}")), g).expect("room");
        }
    }
    assert_eq!(ds.len(), DESK_DRAFTS_MAX);
    assert_eq!(ds.take("s8", &block(&st, "n0"), g).err(), Some(DESK_FULL));
    let each = std::mem::size_of::<(String, Draft)>() + 8 + DRAFT_MAX_BYTES + BLOCK_MAX_OWNED + 16;
    assert!(ds.bytes() <= DESK_DRAFTS_MAX * each, "{} > {}", ds.bytes(), DESK_DRAFTS_MAX * each);
}

/// A draft's own bytes stay within 8 KiB: each label is cut at B1's cap
/// (100 characters), and the change that would pass 8 KiB is refused and
/// taken back (20 labels of 100 four-byte characters don't fit), the
/// draft never past it. The base block is charged at B2's ceiling.
#[test]
fn a_drafts_own_bytes_stay_within_its_cap() {
    let steps: Vec<Value> = (0..20).map(|k| json!({"id":format!("s{k}"),"label":format!("step {k}"),"state":"todo"})).collect();
    let (st, g) = board(&[json!([{"op":"put","id":"plan","kind":"plan","body":{"steps":steps}}])]);
    let mut d = take(&st, g, "plan");
    let wide = "𠀀".repeat(cards_spec::block::STEP_LABEL_MAX + 5);
    let mut refused = 0;
    for k in 0..20 {
        match d.rename(&id(&format!("s{k}")), &wide) {
            Ok(()) => {}
            Err(e) => {
                assert_eq!(e, DRAFT_FULL);
                refused += 1;
            }
        }
        assert!(d.own_bytes() <= DRAFT_MAX_BYTES, "{} after {k}", d.own_bytes());
    }
    let crate::drafts::Intents::Plan(p) = &d.intents else { panic!("a plan") };
    assert!(p.label.values().all(|l| l.chars().count() == cards_spec::block::STEP_LABEL_MAX), "cut at B1's cap");
    assert_eq!(p.label.len() + refused, 20);
    assert!(refused > 0, "the cap refuses before 20 labels of 400 bytes");
    assert!(d.bytes() >= d.own_bytes() + BLOCK_MAX_OWNED, "the base block charged at B2's ceiling");
}
