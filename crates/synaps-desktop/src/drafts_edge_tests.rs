//! The draft model at its edges (Shady #114, round 1): the cap at every
//! change that grows a draft (P2-1), a bound `u-` step is `B`'s (P2-3),
//! a board without the bound board says nothing of a block it doesn't
//! draw (P2-5), a held block kept where it was taken (P2-6), and a
//! session's drafts following it to a new id (P2-4).

use std::collections::HashMap;
use std::sync::Arc;

use cards_spec::block::{Generation, ItemId, Ref};
use cards_spec::board::{Board, Code, Key, State};
use serde_json::{Value, json};
use synaps_client::blocks::{RowBlocks, Shown};

use super::*;

/// A history after the first message `first`: each entry an agent's call
/// of those ops (a receipt binds it), or a user's message (an edit's).
fn history(first: &str, entries: &[Value]) -> Vec<Value> {
    let mut m = vec![json!({"role": "user", "content": first})];
    for (k, e) in entries.iter().enumerate() {
        if let Some(text) = e.as_str() {
            m.push(json!({"role": "user", "content": text}));
            continue;
        }
        let id = format!("toolu_{k}");
        m.push(json!({"role": "assistant", "content": [{"type": "tool_use", "id": id, "name": "cards_block", "input": {"ops": e}}]}));
        m.push(json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": id,
            "content": synaps_client::synthetic::block_receipt(e)}]}));
    }
    m
}

fn board(entries: &[Value]) -> (State, Generation) {
    let (b, events) = Board::of_history(&history("go", entries));
    (b.bound(&events), b.generation())
}

fn id(s: &str) -> ItemId {
    ItemId::parse(s).expect("an item id")
}

fn r(s: &str) -> Ref {
    Ref::parse(s).expect("a block id")
}

fn block(st: &State, name: &str) -> Arc<cards_spec::board::Block> {
    Arc::new(st.block(&r(name)).expect("live").clone())
}

fn take(st: &State, g: Generation, name: &str) -> Draft {
    Draft::of(block(st, name), g).expect("editable")
}

fn line2(text: &str) -> &str {
    text.split('\n').nth(1).expect("line 2")
}

// ── P2-1: the cap at every change that grows a draft ──────────────────

/// A plan of 20 todo steps, `s0` … `s19` (B1's most).
fn twenty() -> Value {
    let steps: Vec<Value> = (0..20).map(|k| json!({"id": format!("s{k}"), "label": format!("S{k}"), "state": "todo"})).collect();
    json!([{"op":"put","id":"big","kind":"plan","title":"Twenty","body":{"steps": steps}}])
}

/// A label of about `b` bytes: four-byte characters, then ASCII.
fn label(b: usize) -> String {
    "𝄞".repeat(b / 4) + &"a".repeat(b % 4)
}

fn room(d: &Draft) -> usize {
    DRAFT_MAX_BYTES.saturating_sub(d.own_bytes())
}

/// What a strike and a stop of an untouched step add to a draft of
/// `big` (bytes): the map entry each charges.
fn charges() -> (usize, usize) {
    let (st, g) = board(&[twenty()]);
    let mut d = take(&st, g, "big");
    let at = d.own_bytes();
    d.strike(&id("s19")).expect("room");
    let strike = d.own_bytes() - at;
    let at = d.own_bytes();
    d.stop(&id("s18")).expect("room");
    (strike, d.own_bytes() - at)
}

/// A draft of `big` filled by accepted renames until its room is one
/// `fits` takes, steps 18 and 19 untouched.
fn filled(fits: impl Fn(usize) -> bool) -> Draft {
    let (st, g) = board(&[twenty()]);
    let mut d = take(&st, g, "big");
    let mut k = 0;
    while k < 17 && d.rename(&id(&format!("s{k}")), &label(400)).is_ok() {
        k += 1;
    }
    assert!(k > 0 && k < 17, "the cap is met within the plan ({k} labels)");
    for b1 in (0..=400).rev() {
        let mut x = d.clone();
        if x.rename(&id(&format!("s{}", k - 1)), &label(b1)).is_err() {
            continue;
        }
        for b2 in 0..=400 {
            let mut y = x.clone();
            if y.rename(&id(&format!("s{k}")), &label(b2)).is_err() {
                break;
            }
            if fits(room(&y)) {
                return y;
            }
        }
    }
    panic!("no fill found");
}

/// Shady #114 P2-1: a strike and a stop add a map entry each, as a label
/// does. Filled by labels to less room than that entry, the strike and
/// the stop of an untouched step are refused (the draft's cap), and the
/// draft is as it was: its intents, its serial, within its cap.
#[test]
fn a_strike_or_a_stop_past_the_cap_is_refused_and_leaves_the_draft_as_it_was() {
    let (strike, stop) = charges();
    assert!(strike > 0 && stop > 0, "each charges its entry: {strike}, {stop}");
    let mut d = filled(|r| r < strike.min(stop));
    let (intents, serial) = (d.intents.clone(), d.serial);
    assert!(d.own_bytes() <= DRAFT_MAX_BYTES);
    assert_eq!(d.strike(&id("s19")), Err(DRAFT_FULL), "a strike past the cap ({} bytes of room)", room(&d));
    assert_eq!((&d.intents, d.serial), (&intents, serial), "the strike left nothing");
    assert!(d.own_bytes() <= DRAFT_MAX_BYTES, "{} > {DRAFT_MAX_BYTES}", d.own_bytes());
    assert_eq!(d.stop(&id("s19")), Err(DRAFT_FULL), "a stop past the cap");
    assert_eq!((&d.intents, d.serial), (&intents, serial), "the stop left nothing");
    assert!(d.own_bytes() <= DRAFT_MAX_BYTES);
    // A change taken back at the cap is taken: it leaves less.
    let renamed = d.own_bytes();
    assert!(d.rename(&id("s0"), "S0").is_ok(), "a label back to B's: no intent, less");
    assert!(d.own_bytes() < renamed);
}

/// Exactly the room a strike takes: it is taken, the draft at its cap
/// to the byte; struck again (taken back) and a stop taken back, at the
/// cap, are taken too: each leaves less.
#[test]
fn a_strike_that_fits_exactly_is_taken_and_one_taken_back_at_the_cap_too() {
    let (strike, _) = charges();
    let mut d = filled(|r| r == strike);
    assert_eq!(d.strike(&id("s19")), Ok(()), "exactly the room it takes");
    assert_eq!(d.own_bytes(), DRAFT_MAX_BYTES, "at the cap, to the byte");
    let serial = d.serial;
    assert_eq!(d.strike(&id("s19")), Ok(()), "struck again: taken back, at the cap");
    assert!(d.own_bytes() < DRAFT_MAX_BYTES && d.serial > serial);
    // A stop that fits, then taken back.
    let mut d = filled(|r| r == strike);
    assert_eq!(d.stop(&id("s19")), Ok(()));
    assert_eq!(d.own_bytes(), DRAFT_MAX_BYTES);
    assert_eq!(d.stop(&id("s19")), Ok(()), "a stop taken back at the cap");
    assert!(d.own_bytes() < DRAFT_MAX_BYTES);
}

/// A proposal's keep and drop go through the same cap (within it here:
/// six hunks at most can't fill a draft), and a label is kept at its
/// length: what the draft counts is what it holds.
#[test]
fn a_hunk_goes_through_the_cap_and_a_label_is_kept_at_its_length() {
    let (st, g) = board(&[json!([{"op":"put","id":"d","kind":"diff","body":{"file":"a.rs","state":"proposed","hunks":[
        {"id":"h1","lines":["+a"],"state":"open"}]}}])]);
    let mut d = take(&st, g, "d");
    let (at, serial) = (d.own_bytes(), d.serial);
    d.hunk(&id("h1"), true).expect("a proposal");
    assert!(d.own_bytes() > at && d.serial == serial + 1, "counted");
    let (st, g) = board(&[twenty()]);
    let mut d = take(&st, g, "big");
    d.rename(&id("s0"), &"é".repeat(150)).expect("room");
    let Intents::Plan(p) = &d.intents else { panic!("a plan") };
    let l = &p.label[&id("s0")];
    assert_eq!((l.chars().count(), l.capacity()), (100, 200), "cut at 100 characters, kept at its length");
}

// ── P2-3: a bound `u-` step is B's ────────────────────────────────────

fn plan3() -> Value {
    json!([{"op":"put","id":"plan","kind":"plan","title":"Ship","body":{"steps":[
        {"id":"fix","label":"Fix","state":"todo"},{"id":"test","label":"Test","state":"todo"},{"id":"deploy","label":"Deploy","state":"todo"}]}}])
}

/// The plan after an earlier edit that added `u-old` after `after` (first
/// if none), handed back and bound: `u-old` is one of B's steps now. The
/// edit's text with it.
fn with_old(after: Option<&str>) -> (State, Generation, ItemId, String) {
    let (st, g) = board(&[plan3()]);
    let mut d = take(&st, g, "plan");
    let old = ItemId::mint(0x0123_4567);
    d.add(after.map(id).as_ref(), old.clone(), "Old step").expect("room");
    let edit = d.check(&st).message.expect("an edit");
    let (now, g2) = board(&[plan3(), Value::String(edit.clone())]);
    assert_eq!(g, g2);
    let Body::Plan(p) = &now.block(&r("plan")).expect("live").body else { panic!("a plan") };
    assert!(p.steps.iter().any(|s| s.id == old), "bound: {:?}", p.steps.iter().map(|s| s.id.as_str()).collect::<Vec<_>>());
    (now, g, old, edit)
}

fn order_of(text: &str) -> Vec<String> {
    let v: Value = serde_json::from_str(line2(text)).expect("line 2 is JSON");
    v["order"]["steps"].as_array().map(|a| a.iter().map(|s| s.as_str().unwrap_or_default().to_string()).collect()).unwrap_or_default()
}

/// Shady #114 P2-3: taken untouched, a plan holding an earlier edit's
/// `u-` step has no change (one Esc lets it go), first or in the middle.
#[test]
fn an_untouched_take_of_a_plan_with_a_bound_u_step_has_no_change() {
    for after in [None, Some("fix")] {
        let (st, g, ..) = with_old(after);
        let d = take(&st, g, "plan");
        assert!(!d.changed(), "untouched ({after:?})");
        assert_eq!(d.check(&st).off.as_deref(), Some("no changes yet"), "{after:?}");
        assert!(d.parts(&|_| true).as_object().is_some_and(serde_json::Map::is_empty), "no patch: {}", d.parts(&|_| true));
    }
}

/// A reorder names every step of B in the order it sends, the bound `u-`
/// step too, first and in the middle: the patch the pre-check checked is
/// the one line 2 writes.
#[test]
fn a_reorder_names_the_bound_u_step_in_the_order_it_sends() {
    let (st, g, old, _) = with_old(None);
    let mut d = take(&st, g, "plan");
    assert!(d.shift(&id("deploy"), -1));
    let text = d.check(&st).message.expect("an edit");
    assert_eq!(order_of(&text), [old.as_str(), "fix", "deploy", "test"], "{text}");
    assert!(!line2(&text).contains("append"), "nothing added: {text}");
    assert!(text.split('\n').next().is_some_and(|l| l.contains(&format!("order {}, fix, deploy, test", old.as_str()))), "{text}");
    let (st, g, old, _) = with_old(Some("fix"));
    let mut d = take(&st, g, "plan");
    assert!(d.shift(&old, -1), "the bound u- step moved up, first");
    let text = d.check(&st).message.expect("an edit");
    assert_eq!(order_of(&text), [old.as_str(), "fix", "test", "deploy"], "{text}");
}

/// A new step the pre-check refuses (the plan is full now) is left out of
/// the order; the bound `u-` step stays in it, where the user put it.
#[test]
fn a_refused_addition_leaves_the_bound_u_step_in_the_order() {
    let (st, g, old, edit) = with_old(None);
    let mut d = take(&st, g, "plan");
    let new = ItemId::mint(0x0765_4321);
    d.add(Some(&id("fix")), new.clone(), "New step").expect("room");
    assert!(d.shift(&id("deploy"), -1), "and deploy before test");
    let more: Vec<Value> = (0..16).map(|k| json!({"id": format!("m{k}"), "label": "More", "state": "todo"})).collect();
    let full = json!([{"op":"patch","id":"plan","append":{"steps": more}}]);
    let (now, _) = board(&[plan3(), Value::String(edit), full]);
    let c = d.check(&now);
    assert!(c.fails.iter().any(|(k, code)| matches!(k, Key::Item(_)) && *code == Code::Full), "the addition refused: {c:?}");
    let text = c.message.expect("the order still goes");
    let o = order_of(&text);
    assert!(!o.iter().any(|s| s == new.as_str()), "the refused step isn't named: {o:?}");
    assert_eq!(&o[..4], [old.as_str(), "fix", "deploy", "test"], "{text}");
}

// ── P2-5: without the bound board, a block it doesn't draw says nothing ─

/// A board as published: rows anchored at row 7 drawing `shown`, the
/// bound board `st` kept or not.
fn drawn(g: Generation, shown: Vec<Shown>, st: Option<State>) -> Drawn {
    let mut rows = HashMap::new();
    rows.insert(7u64, Arc::new(RowBlocks { call: None, blocks: shown, run: None }));
    Drawn { generation: Some(g), rows, board: st.map(Arc::new), ..Default::default() }
}

fn shown(b: &Arc<cards_spec::board::Block>, bound: Option<&Arc<cards_spec::board::Block>>) -> Shown {
    Shown { block: b.clone(), bound: bound.cloned(), members: Vec::new() }
}

/// Shady #114 P2-5: a board published before the bound board is kept
/// (an earlier job's), its live overlay hiding the block a pending
/// `remove` names, ends no draft: the bound board can't say yet. Nor does
/// a pending `replace`, or a stale publication after the bound board.
/// Rolled back, the same draft is editable; a committed remove ends it,
/// for good.
#[test]
fn without_the_bound_board_a_block_it_doesnt_draw_ends_no_draft() {
    let (st, g) = board(&[plan3()]);
    let b = block(&st, "plan");
    let mut ds = Drafts::default();
    let key = {
        let d = ds.take("s", &b, g).expect("room");
        d.strike(&id("fix")).expect("todo");
        d.key
    };
    let ended = |ds: &Drafts| ds.by_key(key).map(|(_, d)| d.ended);
    let hidden = drawn(g, Vec::new(), None);
    assert!(!ds.judge("s", &hidden), "a pending remove hides it: nothing ended");
    assert_eq!(ended(&ds), Some(None), "held");
    let (replaced, _) =
        board(&[plan3(), json!([{"op":"replace","id":"plan","kind":"plan","body":{"steps":[{"id":"a","label":"A","state":"todo"}]}}])]);
    // A pending replace: drawn as its new incarnation, no bound copy of it
    // (the client draws a bound copy of the same incarnation only).
    assert!(!ds.judge("s", &drawn(g, vec![shown(&block(&replaced, "plan"), None)], None)), "a pending replace");
    assert_eq!(ended(&ds), Some(None));
    // Rolled back: drawn again, bound as taken: the same draft, editable.
    assert!(!ds.judge("s", &drawn(g, vec![shown(&b, Some(&b))], None)));
    ds.by_key_mut(key).expect("the same draft").strike(&id("test")).expect("editable");
    // The bound board, then a stale publication without it: still held.
    assert!(!ds.judge("s", &drawn(g, vec![shown(&b, Some(&b))], Some(st.clone()))));
    assert!(!ds.judge("s", &hidden));
    assert_eq!(ended(&ds), Some(None), "unknown stays unknown");
    // Committed: gone, for good.
    let (removed, _) = board(&[plan3(), json!([{"op":"remove","id":"plan"}])]);
    assert!(ds.judge("s", &drawn(g, Vec::new(), Some(removed))));
    assert_eq!(ended(&ds), Some(Some(Fate::Gone)));
    assert!(!ds.judge("s", &drawn(g, vec![shown(&b, Some(&b))], Some(st))), "ended for good");
    assert_eq!(ended(&ds), Some(Some(Fate::Gone)));
}

// ── P2-6: a held block kept where it was taken ────────────────────────

fn rows_of(d: &Drawn, item: u64) -> Vec<String> {
    d.rows.get(&item).map(|r| r.blocks.iter().map(|s| s.block.id.to_string()).collect()).unwrap_or_default()
}

/// Shady #114 P2-6: the board the desk draws keeps a held block where it
/// was taken: an agent's move to another anchor, to the rail, into a
/// group, waits (none drawn twice); in its place, or gone, nothing
/// changes. What the board says of the block is the board's.
#[test]
fn a_held_block_is_drawn_where_it_was_taken() {
    let (st, _) = board(&[json!([
        {"op":"put","id":"a","kind":"note","body":{"text":"A"}},
        {"op":"put","id":"p","kind":"plan","body":{"steps":[{"id":"s","label":"S","state":"todo"}]}},
        {"op":"put","id":"q","kind":"note","body":{"text":"Q"}}])]);
    let sh = |n: &str| shown(&block(&st, n), Some(&block(&st, n)));
    let mut taken = Drawn::default();
    taken.rows.insert(1, Arc::new(RowBlocks { call: None, blocks: vec![sh("a"), sh("p")], run: None }));
    taken.rows.insert(2, Arc::new(RowBlocks { call: None, blocks: vec![sh("q")], run: None }));
    let place = Place::of(&taken, &r("p")).expect("drawn");
    assert_eq!(place, Place::Row { item: 1, at: 1, after: Some(r("a")) });
    let holds = vec![(r("p"), place)];
    assert!(hold_places(&taken, &holds).is_none(), "in its place: as it is");
    // Moved to row 2's anchor.
    let mut moved = taken.clone();
    moved.rows.insert(1, Arc::new(RowBlocks { call: None, blocks: vec![sh("a")], run: None }));
    moved.rows.insert(2, Arc::new(RowBlocks { call: None, blocks: vec![sh("q"), sh("p")], run: None }));
    let kept = hold_places(&moved, &holds).expect("put back");
    assert_eq!((rows_of(&kept, 1), rows_of(&kept, 2)), (vec!["a".to_string(), "p".into()], vec!["q".to_string()]));
    // To the rail.
    let mut railed = moved.clone();
    railed.rows.insert(2, Arc::new(RowBlocks { call: None, blocks: vec![sh("q")], run: None }));
    railed.rail[0].push(sh("p"));
    railed.rail_rows[0].push(Some(1));
    let kept = hold_places(&railed, &holds).expect("put back");
    assert_eq!(rows_of(&kept, 1), ["a", "p"]);
    assert!(kept.rail[0].is_empty() && kept.rail_rows[0].is_empty(), "not in the rail too");
    // Into a group, with q.
    let mut grouped = railed.clone();
    grouped.rail[0].clear();
    grouped.rail_rows[0].clear();
    let g = Shown { block: block(&st, "q"), bound: None, members: vec![sh("q"), sh("p")] };
    grouped.rows.insert(2, Arc::new(RowBlocks { call: None, blocks: vec![g], run: None }));
    let kept = hold_places(&grouped, &holds).expect("put back");
    assert_eq!(rows_of(&kept, 1), ["a", "p"]);
    assert_eq!(kept.rows[&2].blocks[0].members.len(), 1, "out of the group");
    // `a` removed: p is first there now, in its place.
    let mut alone = taken.clone();
    alone.rows.insert(1, Arc::new(RowBlocks { call: None, blocks: vec![sh("p")], run: None }));
    assert!(hold_places(&alone, &holds).is_none(), "its list, the block before it gone");
    // Gone: nothing to hold.
    let mut gone = taken.clone();
    gone.rows.insert(1, Arc::new(RowBlocks { call: None, blocks: vec![sh("a")], run: None }));
    assert!(hold_places(&gone, &holds).is_none());
    // A rail section taken stays in its slot.
    let mut in_rail = Drawn::default();
    in_rail.rail[1] = vec![sh("q"), sh("p")];
    in_rail.rail_rows[1] = vec![Some(4), Some(5)];
    let place = Place::of(&in_rail, &r("p")).expect("drawn");
    assert_eq!(place, Place::Rail { slot: 1, at: 1, row: Some(5), after: Some(r("q")) });
    let mut out = Drawn::default();
    out.rail[1] = vec![sh("q")];
    out.rail_rows[1] = vec![Some(4)];
    out.rows.insert(9, Arc::new(RowBlocks { call: None, blocks: vec![sh("p")], run: None }));
    let kept = hold_places(&out, &[(r("p"), place)]).expect("put back");
    assert_eq!((kept.rail[1].len(), kept.rail_rows[1].clone()), (2, vec![Some(4), Some(5)]));
    assert!(rows_of(&kept, 9).is_empty());
}

// ── P2-4: a session's drafts follow it ────────────────────────────────

/// Shady #114 P2-4: `S` follows to `T`: every draft is `T`'s, none left
/// under `S`. One of the same incarnation as `T`'s: an unchanged one
/// goes (nothing lost); both changed, the one that came is doubled, in
/// the dock, its key its own (a discard of it leaves `T`'s).
#[test]
fn a_follow_takes_every_draft_and_drops_none_with_changes() {
    let (st, g) = board(&[
        plan3(),
        json!([{"op":"put","id":"n","kind":"note","body":{"text":"N"}},
        {"op":"put","id":"m","kind":"note","body":{"text":"M"}}]),
    ]);
    let mut ds = Drafts::default();
    ds.take("S", &block(&st, "plan"), g).expect("room").strike(&id("fix")).expect("todo");
    let n_s = ds.take("S", &block(&st, "n"), g).expect("room").key;
    ds.take("S", &block(&st, "m"), g).expect("room").note(false, "M, edited").expect("room");
    let p_t = {
        let d = ds.take("T", &block(&st, "plan"), g).expect("room");
        d.stop(&id("test")).expect("todo");
        d.key
    };
    let n_t = {
        let d = ds.take("T", &block(&st, "n"), g).expect("room");
        d.note(false, "N, edited").expect("room");
        d.key
    };
    let gone = ds.follow("S", "T");
    assert_eq!(ds.count("S"), 0, "none left under S");
    assert_eq!(gone, vec![n_s], "S's unchanged n went: T's has the user's text");
    assert_eq!(ds.count("T"), 4, "T's two, S's plan (doubled) and m");
    let doubled: Vec<&Draft> = ds.of("T").filter(|d| d.ended == Some(Fate::Doubled)).collect();
    assert_eq!(doubled.len(), 1);
    assert_eq!(doubled[0].id, r("plan"));
    assert_eq!(Fate::Doubled.says().as_deref(), Some(DOUBLED));
    assert!(!Fate::in_row(Some(Fate::Doubled)), "in the dock");
    assert_eq!(ds.in_row("T", &r("plan")).map(|d| d.key), Some(p_t), "T's own plan draws in its row");
    assert!(ds.by_key(n_t).is_some_and(|(s, d)| s == "T" && d.ended.is_none()));
    let dk = doubled[0].key;
    assert!(ds.discard_key(dk));
    assert!(ds.by_key(p_t).is_some(), "T's plan kept");
    let keys: Vec<u64> = ds.of("T").map(|d| d.key).collect();
    assert_eq!(keys.len(), 3);
    assert!(keys.iter().all(|k| keys.iter().filter(|x| *x == k).count() == 1), "keys unique: {keys:?}");
    // Past 8 there: none dropped, a take refused.
    let notes: Vec<Value> = (0..9).map(|k| json!({"op":"put","id":format!("x{k}"),"kind":"note","body":{"text":"x"}})).collect();
    let (st2, g2) = board(&[Value::Array(notes)]);
    let mut ds = Drafts::default();
    for k in 0..5 {
        ds.take("S", &block(&st2, &format!("x{k}")), g2).expect("room");
    }
    for k in 5..8 {
        ds.take("T", &block(&st2, &format!("x{k}")), g2).expect("room");
    }
    ds.take("T", &block(&st2, "x0"), g2).expect("room").note(false, "x, edited").expect("room");
    ds.take("S", &block(&st2, "x0"), g2).expect("the same").note(false, "x, mine").expect("room");
    ds.follow("S", "T");
    assert_eq!(ds.count("T"), 9, "none dropped");
    assert_eq!(ds.take("T", &block(&st2, "x8"), g2).err(), Some(FULL), "past 8: a take refused");
}
