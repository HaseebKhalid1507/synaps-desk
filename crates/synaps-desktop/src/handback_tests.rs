//! The hand-back model's own tests (D8b, blocks.md §3.7, §1.8): what a
//! row says the board did, what reads as an edit, the memos' bounds, and
//! a refusal given to the memo whose oldest sending entry is older.

use std::sync::Arc;
use std::time::{Duration, Instant};

use cards_spec::block::message::{Edit, Trailer};
use cards_spec::block::{Ref, Tag};
use cards_spec::board::{Board, Code, EditOutcome, EditResult, Pos, State};
use serde_json::{Value, json};
use synaps_client::Transcript;
use synaps_client::blocks::Drawn;

use super::*;

fn plan(id: &str) -> Value {
    json!([{"op":"put","id":id,"kind":"plan","title":"Plan","body":{"steps":[
        {"id":"fix","label":"Fix","state":"todo"},{"id":"deploy","label":"Deploy","state":"todo"}]}}])
}

/// The bound board of `rounds` (one call each), whole.
fn board(rounds: &[Value]) -> State {
    let mut m = vec![json!({"role": "user", "content": "go"})];
    for (k, ops) in rounds.iter().enumerate() {
        let id = format!("toolu_{k}");
        m.push(json!({"role": "assistant", "content": [{"type": "tool_use", "id": id, "name": "cards_block", "input": {"ops": ops}}]}));
        m.push(json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": id,
            "content": synaps_client::synthetic::block_receipt(ops)}]}));
    }
    let (b, events) = Board::of_history(&m);
    b.bound(&events)
}

fn drawn(st: State) -> Drawn {
    Drawn { board: Some(Arc::new(st)), ..Default::default() }
}

fn edit(st: &State, id: &str, set: Value) -> Edit {
    let b = st.block(&Ref::parse(id).unwrap()).unwrap();
    Edit::new(b.id.clone(), b.rev, b.tag, &json!({"set": set})).unwrap()
}

fn line1(words: &str) -> Line1 {
    Line1 { title: Some("Plan".into()), back_to: None, pieces: vec![words.into()] }
}

/// §3.7's application layer, as a row says it: applied whole, or `N of
/// M` and why each other didn't (each reason once), the paths the agent
/// overwrote since; a rewritten, gone or full incarnation applies nothing;
/// one compacted away was delivered.
#[test]
fn a_rows_outcome_in_words() {
    let st = board(&[plan("p")]);
    let e = edit(&st, "p", json!({"steps.fix.state":"skipped","steps.deploy.state":"skipped","steps.deploy.stop":true}));
    let o = |result, codes: Vec<Code>| EditOutcome { at: Pos::event(9), id: e.id.clone(), result, codes };
    let words = |o: &EditOutcome, c| outcome_words(o, Some(&e), c);
    let k = keys(&e);
    assert_eq!(k.len(), 3);
    assert_eq!(words(&o(EditResult::Applied, vec![Code::Applied; 3]), false), "applied");
    assert_eq!(
        words(&o(EditResult::Applied, vec![Code::Applied, Code::Unchanged, Code::Applied]), false),
        "applied",
        "already so: applied"
    );
    let ran = o(EditResult::Applied, vec![Code::AlreadyRan, Code::AlreadyRan, Code::Applied]);
    let at: Vec<String> = k.iter().map(|k| format!("{k:?}")).collect();
    assert!(at[0].contains("steps.deploy.state") && at[1].contains("steps.deploy.stop"), "B2's key order: {at:?}");
    assert_eq!(words(&ran, false), "applied 1 of 3 · deploy already ran", "each reason once");
    let two = o(EditResult::Applied, vec![Code::AlreadyRan, Code::Applied, Code::NotThere]);
    assert_eq!(words(&two, false), "applied 1 of 3 · deploy already ran · fix is gone");
    assert_eq!(
        words(&o(EditResult::Applied, vec![Code::Overwritten, Code::Applied, Code::Applied]), false),
        "applied · 1 overwritten by the agent"
    );
    assert_eq!(words(&o(EditResult::Rewritten, vec![]), false), "not applied: the block was rewritten");
    assert_eq!(words(&o(EditResult::Gone, vec![]), false), "not applied: the block is gone");
    assert_eq!(words(&o(EditResult::Gone, vec![]), true), COMPACTED_AWAY);
    assert_eq!(words(&o(EditResult::Revisions, vec![]), false), "not applied: replace the block or use a new id");
}

/// A row reads as a hand-back only if all of its body is the grammar
/// (§3.3): notice lines may follow; a look-alike with anything else is
/// ordinary text.
#[test]
fn only_the_grammar_reads_as_an_edit() {
    let st = board(&[plan("p")]);
    let text =
        cards_spec::block::message::build(&edit(&st, "p", json!({"steps.fix.state":"skipped"})), &line1("skip fix").summary()).unwrap();
    assert!(is_edit(&text));
    assert!(is_edit(&format!("{text}\n[block q v1 dismissed]")), "a notice after");
    assert!(!is_edit(&format!("{text}\nand more")), "text after");
    assert!(!is_edit("[block p edited r1] Plan → skip fix"), "line 1 alone");
    assert!(!is_edit("skip fix"));
}

/// §1.8: a session's memo keeps 32 entries (B4's); the desk keeps memos
/// for 16 sessions: a 17th's hand-back is refused while each of the 16
/// has one out, and once one has none that can't go, that memo goes.
/// What they keep is charged, each entry at its ceiling.
#[test]
fn sixteen_sessions_of_hand_backs_and_a_seventeenth_waits() {
    let st = board(&[plan("p")]);
    let d = drawn(st.clone());
    let t = Transcript::new();
    let mut h = HandBacks::default();
    let now = Instant::now();
    let e = edit(&st, "p", json!({"steps.fix.state":"skipped"}));
    for k in 0..SESSIONS_MAX {
        let s = format!("s{k}");
        h.may_hand_back(&s, &e, &line1("skip fix"), &t, Some(&d)).expect("room");
        h.hand_back(&s, &e, &line1("skip fix"), &Trailer::default(), &t, Some(&d), now).expect("sent");
    }
    assert_eq!(h.len(), SESSIONS_MAX);
    assert!(h.bytes() >= SESSIONS_MAX * ENTRY_MAX_BYTES, "charged: {}", h.bytes());
    assert_eq!(h.may_hand_back("s16", &e, &line1("skip fix"), &t, Some(&d)).err().as_deref(), Some(SESSIONS_FULL));
    assert_eq!(h.hand_back("s16", &e, &line1("skip fix"), &Trailer::default(), &t, Some(&d), now).err().as_deref(), Some(SESSIONS_FULL));
    // One edit in flight per incarnation, as B4 says.
    assert_eq!(h.may_hand_back("s0", &e, &line1("skip fix"), &t, Some(&d)).err().as_deref(), Some(SENDING));
    // s0's in the chat: its memo can go.
    h.on("s0", Heard::Echo { text: &h.latest("s0", &e.id, e.of).unwrap().text.clone(), driver: false }, now);
    h.on("s0", Heard::Delivered { text: &h.latest("s0", &e.id, e.of).unwrap().text.clone() }, now);
    assert_eq!(h.latest("s0", &e.id, e.of).map(|x| x.state), Some(cards_spec::memo::State::InChat));
    h.hand_back("s16", &e, &line1("skip fix"), &Trailer::default(), &t, Some(&d), now).expect("room made");
    assert_eq!(h.len(), SESSIONS_MAX);
    assert!(h.latest("s0", &e.id, e.of).is_none(), "the settled memo went");
}

/// §3.7, "Two memos, one session": a `Refused` names no text; it is the
/// memo's whose oldest sending entry is older: the card's answer sent
/// first takes it, then the hand-back.
#[test]
fn a_refusal_is_the_older_sending_entrys() {
    let st = board(&[plan("p")]);
    let d = drawn(st.clone());
    let t = Transcript::new();
    let mut m = crate::deliver::Memos::default();
    let t0 = Instant::now();
    let e = edit(&st, "p", json!({"steps.fix.state":"skipped"}));
    m.edits.hand_back("s", &e, &line1("skip fix"), &Trailer::default(), &t, Some(&d), t0).unwrap();
    let body = m.edits.latest("s", &e.id, e.of).unwrap().text.clone();
    m.on("s", Heard::Refused { reason: "busy" }, t0 + Duration::from_millis(5));
    assert_eq!(m.edits.get("s", &body).map(|x| x.state), Some(cards_spec::memo::State::NotSent), "the only one sending");
    let _: Tag = e.of;
}
