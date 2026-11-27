//! D6b wired, in the window ([`crate::rail_wishes`]): a rail section's
//! `×` and `⇅`, the footer's `show`, the composer's chip, the notice
//! riding the next message and settling once its history lands, and a
//! compaction's soft dismissal drawn as its tombstone, `u` ending it.
//!
//! The helpers are `ui_tests`' own, from [`super`] (`test_kit`), and D5's
//! and D6a's window helpers.

use gpui_kit::{AnyWindowHandle, Entity, TestAppContext, point, px};
use serde_json::json;
use synaps_client::blocks::Ref;

#[cfg(unix)]
use super::answer_tests::land;
use super::answer_tests::{attachment, has, heard};
use super::blocks_ui_tests::{board, click, drawn, mouse, open_session};
use super::{open_at, wait};
use crate::app::AppView;
use crate::tabs::Place;

type W = (AnyWindowHandle, Entity<AppView>);

/// The rail demo session (`synthetic::rail_history`), its board drawn.
fn rail(cx: &mut TestAppContext) -> W {
    let (win, view, _events) = open_at(cx, (1280., 2400.), crate::theme::Panels::default().text_size);
    open_session(cx, win, &view, crate::demo::RAIL, Place::Current);
    (win, view)
}

fn row(name: &str) -> u64 {
    crate::blocks::row_id(&Ref::parse(name).expect("an id"))
}

/// Section `name` in the rail as laid out (drawn once a leaving glide is
/// over).
fn has_sec(cx: &mut TestAppContext, view: &Entity<AppView>, name: &str) -> bool {
    let r = Ref::parse(name).expect("an id");
    view.read_with(cx, |v, _| v.rail_layout().is_some_and(|(_, l)| l.section(&r).is_some()))
}

/// Section `name`'s hover action `act` (`agent-x`, `agent-slot`),
/// clicked: the pointer over its header first, so it draws.
fn action(cx: &mut TestAppContext, w: AnyWindowHandle, name: &str, act: &str) {
    let head = drawn(cx, w, format!("agent-head {}", row(name))).center();
    mouse(cx, w, head, "move");
    let at = drawn(cx, w, format!("{act} {}", row(name))).center();
    click(cx, w, at);
}

fn desires(cx: &mut TestAppContext, view: &Entity<AppView>) -> usize {
    view.read_with(cx, |v, _| v.wishes.desires.len())
}

#[cfg(unix)]
fn outbox(cx: &mut TestAppContext, view: &Entity<AppView>) -> Vec<String> {
    view.read_with(cx, |v, _| v.wishes.outbox.clone())
}

/// The whole history replaced by `messages` (a compaction's, as the
/// client hands it over).
fn replace(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, messages: Vec<serde_json::Value>) {
    let (seq, sid) = attachment(cx, view);
    view.update(cx, |v, cx| {
        let update = synaps_client::HistoryUpdate::Replace { messages };
        v.apply_batch(vec![synaps_client::Event::History { attach_seq: seq, session_id: sid, seq: 0, update }], cx);
    });
    cx.run_until_parked();
    super::settle(cx, w, 8);
}

/// The pointer off the panel: the rail's lease ends, and its glides end.
fn away(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    mouse(cx, w, point(px(700.), px(400.)), "move");
    wait(cx, w, view, 1000);
}

#[gpui_kit::test]
fn x_dismisses_a_section_and_show_brings_an_unsent_one_back(cx: &mut TestAppContext) {
    let (w, view) = rail(cx);
    assert!(has_sec(cx, &view, "plan") && has(cx, w, "agent-count 7 sections".into()));
    action(cx, w, "plan", "agent-x");
    away(cx, w, &view);
    assert!(!has_sec(cx, &view, "plan"), "dismissed: the section goes");
    assert!(has(cx, w, "agent-count 6 sections · 1 dismissed".into()));
    assert!(has(cx, w, "agent-footer 1 dismissed · show".into()));
    assert!(has(cx, w, "composer-tells + tells the agent: plan dismissed ×".into()), "the chip says what rides");
    assert_eq!(desires(cx, &view), 1, "written");
    // `show`: never sent, so simply cancelled.
    let show = drawn(cx, w, "agent-footer 1 dismissed · show".into()).center();
    click(cx, w, show);
    away(cx, w, &view);
    assert!(has_sec(cx, &view, "plan") && has(cx, w, "agent-count 7 sections".into()));
    assert!(!has(cx, w, "agent-footer 1 dismissed · show".into()));
    assert!(!has(cx, w, "composer-tells + tells the agent: plan dismissed ×".into()));
    assert_eq!(desires(cx, &view), 0, "nothing to tell");
}

#[gpui_kit::test]
fn move_takes_a_section_to_the_other_slot_and_back(cx: &mut TestAppContext) {
    let (w, view) = rail(cx);
    let end = |cx: &mut TestAppContext| drawn(cx, w, "agent-zone 1".into());
    let in_end = |cx: &mut TestAppContext| {
        let z = end(cx);
        z.contains(&drawn(cx, w, format!("agent-section {}", row("cause"))).origin)
    };
    assert!(!in_end(cx));
    action(cx, w, "cause", "agent-slot");
    away(cx, w, &view);
    assert!(in_end(cx), "moved by the user: the end slot");
    action(cx, w, "cause", "agent-slot");
    away(cx, w, &view);
    assert!(!in_end(cx), "again: the agent's placement");
}

/// The notice rides the next typed message (once its mark is written),
/// the chip's click keeps it back a message, and the history that has it
/// settles the desire: shared now, the section stays dismissed.
#[cfg(unix)]
#[gpui_kit::test]
fn the_notice_rides_the_next_message_and_settles(cx: &mut TestAppContext) {
    let (w, view) = rail(cx);
    action(cx, w, "plan", "agent-x");
    away(cx, w, &view);
    let chip = drawn(cx, w, "composer-tells + tells the agent: plan dismissed ×".into()).center();
    click(cx, w, chip);
    assert!(!has(cx, w, "composer-tells + tells the agent: plan dismissed ×".into()), "kept back");
    view.update(cx, |v, cx| assert!(v.enter("first".into(), cx)));
    board(cx, w, &view);
    assert_eq!(outbox(cx, &view), ["first"], "kept back from this one");
    assert!(has(cx, w, "composer-tells + tells the agent: plan dismissed ×".into()), "the next carries it");
    view.update(cx, |v, cx| assert!(v.enter("second".into(), cx)));
    board(cx, w, &view);
    assert_eq!(outbox(cx, &view), ["first", "second\n[block plan v1 dismissed]"]);
    assert!(!has(cx, w, "composer-tells + tells the agent: plan dismissed ×".into()), "in flight");
    assert_eq!(desires(cx, &view), 1);
    land(
        cx,
        w,
        &view,
        vec![
            json!({"role": "user", "content": "first"}),
            json!({"role": "assistant", "content": [{"type": "text", "text": "ok"}]}),
            json!({"role": "user", "content": "second\n[block plan v1 dismissed]"}),
            json!({"role": "assistant", "content": [{"type": "text", "text": "noted"}]}),
        ],
    );
    board(cx, w, &view);
    assert_eq!(desires(cx, &view), 0, "settled: the history has it");
    assert!(!has_sec(cx, &view, "plan") && has(cx, w, "agent-footer 1 dismissed · show".into()), "shared now");
    // A Show of a shared dismissal is a new version: it rides next.
    let show = drawn(cx, w, "agent-footer 1 dismissed · show".into()).center();
    click(cx, w, show);
    away(cx, w, &view);
    assert!(has_sec(cx, &view, "plan"));
    assert!(has(cx, w, "composer-tells + tells the agent: plan shown ×".into()));
}

/// A turn running: the notice rides a `Steer`, and stays in flight
/// while the turn runs.
#[cfg(unix)]
#[gpui_kit::test]
fn the_notice_rides_a_steer(cx: &mut TestAppContext) {
    let (w, view) = rail(cx);
    action(cx, w, "plan", "agent-x");
    away(cx, w, &view);
    heard(cx, w, &view, vec![json!({"ev": "turn_started", "turn_baseline": 6, "trigger": "user", "user_text": "go on"})]);
    assert!(view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].transcript.busy()));
    view.update(cx, |v, cx| assert!(v.enter("and this".into(), cx)));
    board(cx, w, &view);
    assert_eq!(outbox(cx, &view), ["and this\n[block plan v1 dismissed]"]);
    // A settle while the turn runs (the session's first board seen): the
    // steer not delivered yet, its carrier still in flight. The next steer
    // carries nothing again.
    view.update(cx, |v, _| v.wishes.forget_boards());
    let same = json!([{"op": "patch", "id": "now", "set": {"text": "Running it again"}}]);
    super::blocks_ui_tests::round(cx, w, &view, &[("toolu_w2", same, true)]);
    view.update(cx, |v, cx| assert!(v.enter("more".into(), cx)));
    board(cx, w, &view);
    assert_eq!(outbox(cx, &view).last().map(String::as_str), Some("more"), "in flight: not again");
}

/// A compaction carries the dismissal into the successor softly: the
/// block put again draws as its tombstone, and its click (`u`) ends it.
#[gpui_kit::test]
fn a_compaction_carries_a_dismissal_as_a_tombstone(cx: &mut TestAppContext) {
    let (w, view) = rail(cx);
    action(cx, w, "plan", "agent-x");
    away(cx, w, &view);
    heard(cx, w, &view, vec![json!({"ev": "compaction_started", "source": "manual", "disclosure": ""})]);
    let sid = view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].transcript.journal_id().to_string());
    heard(
        cx,
        w,
        &view,
        vec![
            json!({"ev": "compaction_applied", "previous_session_id": sid, "session_id": "j-next", "chains_advanced": [], "msg_count": 6}),
        ],
    );
    let mut compacted = vec![
        json!({"role": "user", "content": "<context-summary>rail work</context-summary>"}),
        json!({"role": "assistant", "content": "Understood."}),
    ];
    compacted.extend(synaps_client::synthetic::rail_history());
    replace(cx, w, &view, compacted);
    board(cx, w, &view);
    assert_eq!(view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].transcript.journal_id().to_string()), "j-next");
    let tomb = format!("agent-tomb {}", row("plan"));
    assert!(has(cx, w, tomb.clone()), "the block put again: its tombstone");
    assert!(!has(cx, w, "composer-tells + tells the agent: plan dismissed ×".into()), "soft: nothing told");
    let at = drawn(cx, w, tomb.clone()).center();
    click(cx, w, at);
    away(cx, w, &view);
    assert!(!has(cx, w, tomb) && has_sec(cx, &view, "plan"), "u: shown");
}

/// Another desk owns this user's files: this one keeps its changes in
/// memory only, and its rail says so.
#[gpui_kit::test]
fn a_second_desk_says_its_changes_stay_in_memory(cx: &mut TestAppContext) {
    let (w, view) = rail(cx);
    let path = crate::desires::path().expect("the test home");
    std::fs::create_dir_all(path.parent().expect("its folder")).expect("the folder");
    let _other = crate::blockfiles::own(&path).expect("the other desk's lock");
    view.update(cx, |v, cx| v.start_wishes(cx));
    away(cx, w, &view);
    assert!(view.read_with(cx, |v, _| !v.wishes.desires.owns_file()));
    let said = view.read_with(cx, |v, _| v.wishes.said.clone()).expect("said");
    assert!(said.contains("another desk's"), "{said}");
    action(cx, w, "plan", "agent-x");
    away(cx, w, &view);
    assert!(!has_sec(cx, &view, "plan"), "kept in memory: it draws");
}

/// A notice in flight stays so until its history lands: the turn that
/// carried it ends first, and the desire neither settles on the turn's
/// board (only the history's records settle) nor rides again.
#[cfg(unix)]
#[gpui_kit::test]
fn a_notice_in_flight_waits_for_its_history(cx: &mut TestAppContext) {
    let (w, view) = rail(cx);
    action(cx, w, "plan", "agent-x");
    away(cx, w, &view);
    view.update(cx, |v, cx| assert!(v.enter("go".into(), cx)));
    board(cx, w, &view);
    let sent = "go\n[block plan v1 dismissed]";
    assert_eq!(outbox(cx, &view), [sent]);
    heard(
        cx,
        w,
        &view,
        vec![
            json!({"ev": "turn_started", "turn_baseline": 6, "trigger": "user", "user_text": sent}),
            super::answer_tests::llm(json!({"kind": "llm", "llm": "response_start"})),
        ],
    );
    board(cx, w, &view);
    assert!(view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].transcript.busy()));
    heard(
        cx,
        w,
        &view,
        vec![
            super::answer_tests::llm(json!({"kind": "llm", "llm": "text", "text": "noted"})),
            super::answer_tests::llm(json!({"kind": "session", "session": "done"})),
            json!({"ev": "idle"}),
        ],
    );
    board(cx, w, &view);
    assert!(view.read_with(cx, |v, _| !v.model.lives[v.panes.cur.live].transcript.busy()), "the turn is over");
    assert_eq!(desires(cx, &view), 1, "not settled by the turn's board");
    assert!(!has(cx, w, "composer-tells + tells the agent: plan dismissed ×".into()), "still in flight: it rides nothing");
    land(
        cx,
        w,
        &view,
        vec![json!({"role": "user", "content": sent}), json!({"role": "assistant", "content": [{"type": "text", "text": "ok"}]})],
    );
    board(cx, w, &view);
    assert_eq!(desires(cx, &view), 0, "its history: settled");
    assert!(!has_sec(cx, &view, "plan"));
}

/// A settle mid-turn (the session's first board seen while its turn
/// runs): the turn's board has the notice, the history doesn't. Only the
/// history's records settle, and the carrier is still in flight: kept,
/// and it rides nothing.
#[cfg(unix)]
#[gpui_kit::test]
fn a_settle_mid_turn_reads_the_history_and_keeps_the_carrier(cx: &mut TestAppContext) {
    let (w, view) = rail(cx);
    action(cx, w, "plan", "agent-x");
    away(cx, w, &view);
    view.update(cx, |v, cx| assert!(v.enter("go".into(), cx)));
    board(cx, w, &view);
    let sent = "go\n[block plan v1 dismissed]";
    heard(cx, w, &view, vec![json!({"ev": "turn_started", "turn_baseline": 6, "trigger": "user", "user_text": sent})]);
    board(cx, w, &view);
    // What this window knew of the session goes (its tab opened again).
    view.update(cx, |v, _| v.wishes.forget_boards());
    let same = json!([{"op": "patch", "id": "now", "set": {"text": "Running it again"}}]);
    super::blocks_ui_tests::round(cx, w, &view, &[("toolu_w1", same, true)]);
    let p = cards_spec::block::BlockId::parse("plan").expect("an id");
    let (live, history) = view.read_with(cx, |v, _| {
        let d = v.model.lives[v.panes.cur.live].transcript.drawn().expect("a board");
        (d.visibility.dismissed(&p), d.settled.dismissed(&p))
    });
    assert!(live && !history, "the turn's board has it, the history doesn't");
    assert_eq!(desires(cx, &view), 1, "not settled");
    assert!(!has(cx, w, "composer-tells + tells the agent: plan dismissed ×".into()), "in flight: it rides nothing");
}
