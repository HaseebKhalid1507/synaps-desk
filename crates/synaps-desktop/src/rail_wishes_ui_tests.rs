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
use super::blocks_ui_tests::{click, drawn, mouse, open_session};
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

/// The pointer off the panel: the rail's lease ends, and its glides end;
/// the writes it asked for landed.
fn away(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    mouse(cx, w, point(px(700.), px(400.)), "move");
    wait(cx, w, view, 1000);
    written(cx, view);
    super::settle(cx, w, 8);
}

/// Every write the window handed the writer run (its executor runs a
/// test's writer), and what each landed taken, the writes that asks for
/// too.
pub(super) fn written(cx: &mut TestAppContext, view: &Entity<AppView>) {
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| !v.wishes.out()), "every write landed");
}

/// The board drawn, the writes it asked for landed.
fn board(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    written(cx, view);
    super::blocks_ui_tests::board(cx, w, view);
    written(cx, view);
    super::settle(cx, w, 8);
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
    // A Show of a shared dismissal is a new version: it rides next. (What
    // the footer shows changed with the history: a moment first.)
    wait(cx, w, &view, 700);
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
    // Come with the agent's board: just moved a moment (D6a's guard).
    wait(cx, w, &view, 700);
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
    let home = crate::rail_wishes::Home::take(Some(path.clone()), crate::presentation::path());
    view.update(cx, |v, cx| v.start_wishes(home, cx));
    away(cx, w, &view);
    assert!(view.read_with(cx, |v, _| !v.wishes.desires.owns_file()));
    let said = view.read_with(cx, |v, _| v.wishes.memory_only()).expect("said");
    assert!(said.contains("another desk has its files"), "{said}");
    action(cx, w, "plan", "agent-x");
    away(cx, w, &view);
    assert!(!has_sec(cx, &view, "plan"), "kept in memory: it draws");
    // Its notice rides the next message, and that message's echo, the
    // text as sent, confirms it (Shady #103 R3-P2-1).
    #[cfg(unix)]
    {
        view.update(cx, |v, cx| assert!(v.enter("one".into(), cx)));
        board(cx, w, &view);
        let one = "one\n[block plan v1 dismissed]";
        assert_eq!(outbox(cx, &view), [one]);
        assert_eq!(in_flight(cx, &view), ["one"]);
        heard(cx, w, &view, vec![json!({"ev": "turn_started", "turn_baseline": 6, "trigger": "user", "user_text": one})]);
        assert!(in_flight(cx, &view).is_empty(), "its echo confirmed it");
    }
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

// Shady's review of #103, round 3 (baa488a).

/// What the focused pane's store has in flight: each input's body.
#[cfg(unix)]
fn in_flight(cx: &mut TestAppContext, view: &Entity<AppView>) -> Vec<String> {
    view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].in_flight.iter().cloned().collect())
}

/// What the focused pane would give back to its composer, and its text.
#[cfg(unix)]
fn drafts(cx: &mut TestAppContext, view: &Entity<AppView>) -> String {
    view.read_with(cx, |v, cx| format!("{}{}", v.panes.cur.restore.clone().unwrap_or_default(), v.panes.cur.composer.read(cx).value()))
}

/// What `view`'s two files hold, as a restart reads them (none: empty).
#[cfg(unix)]
fn on_disk(cx: &mut TestAppContext, view: &Entity<AppView>) -> (String, String) {
    view.read_with(cx, |v, _| {
        let read = |p: Option<&std::path::Path>| p.and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
        (read(v.wishes.desires.path()), read(v.wishes.pres.path()))
    })
}

fn note(cx: &mut TestAppContext, view: &Entity<AppView>) -> Option<(u64, &'static str)> {
    view.read_with(cx, |v, _| v.agent_rail.lease.note)
}

/// A turn on the rail session (its history is 6 messages): the agent's
/// ops land in it.
fn go(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>) {
    heard(cx, w, view, vec![json!({"ev": "turn_started", "turn_baseline": 6, "trigger": "user", "user_text": "go on"})]);
}

fn ops(cx: &mut TestAppContext, w: AnyWindowHandle, view: &Entity<AppView>, tool: &str, ops: serde_json::Value) {
    super::blocks_ui_tests::round(cx, w, view, &[(tool, ops, true)]);
}

/// R3-P2-1: a message its notices rode is confirmed by its echo, the text
/// as sent: sent at once (its mark written) or once its mark is, a submit
/// or a steer; one whose mark's write failed goes plain, and its echo is
/// that. A detach after gives the composer nothing back.
#[cfg(unix)]
#[gpui_kit::test]
fn a_message_with_its_notices_is_confirmed_by_its_echo(cx: &mut TestAppContext) {
    let (w, view) = rail(cx);
    action(cx, w, "plan", "agent-x");
    away(cx, w, &view);
    view.update(cx, |v, cx| assert!(v.enter("one".into(), cx)));
    board(cx, w, &view);
    let one = "one\n[block plan v1 dismissed]";
    assert_eq!(outbox(cx, &view), [one]);
    assert_eq!(in_flight(cx, &view), ["one"], "in flight, its body to give back");
    heard(cx, w, &view, vec![json!({"ev": "turn_started", "turn_baseline": 6, "trigger": "user", "user_text": one})]);
    assert!(in_flight(cx, &view).is_empty(), "its echo, the text as sent, confirmed it");
    // A steer (the turn runs), waiting for its mark (`sent`) to be written.
    action(cx, w, "cause", "agent-x");
    away(cx, w, &view);
    view.update(cx, |v, cx| assert!(v.enter("two".into(), cx)));
    assert_eq!(in_flight(cx, &view), ["two"], "waiting for its mark, out of the composer");
    board(cx, w, &view);
    let two = outbox(cx, &view).last().cloned().expect("sent");
    assert!(two.starts_with("two\n[block cause v") && two.ends_with(" dismissed]"), "{two}");
    heard(cx, w, &view, vec![json!({"ev": "steered", "text": two, "delivered": true})]);
    assert!(in_flight(cx, &view).is_empty(), "the steer's echo confirmed it");
    // Its mark's write fails: it goes plain, and its echo is that.
    action(cx, w, "now", "agent-x");
    away(cx, w, &view);
    let path = view.read_with(cx, |v, _| v.wishes.desires.path().expect("a file").to_path_buf());
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    view.update(cx, |v, cx| assert!(v.enter("three".into(), cx)));
    board(cx, w, &view);
    assert_eq!(outbox(cx, &view).last().map(String::as_str), Some("three"), "plain: its mark wasn't written");
    heard(cx, w, &view, vec![json!({"ev": "steered", "text": "three", "delivered": true})]);
    assert!(in_flight(cx, &view).is_empty());
    // A detach: nothing sent comes back as a draft.
    let (seq, sid) = attachment(cx, &view);
    view.update(cx, |v, cx| {
        v.apply_batch(vec![synaps_client::Event::Detached { attach_seq: seq, session_id: Some(sid), reason: "gone".into() }], cx)
    });
    cx.run_until_parked();
    assert_eq!(drafts(cx, &view), "", "each went and was confirmed");
}

/// R3-P2-2: a settle and a compaction's carry write themselves: with no
/// click or message after them, a restart reads them in the files.
#[cfg(unix)]
#[gpui_kit::test]
fn a_settle_and_a_compactions_carry_are_written_with_no_click_after(cx: &mut TestAppContext) {
    let (w, view) = rail(cx);
    action(cx, w, "plan", "agent-x");
    away(cx, w, &view);
    view.update(cx, |v, cx| assert!(v.enter("go".into(), cx)));
    board(cx, w, &view);
    let sent = "go\n[block plan v1 dismissed]";
    assert!(on_disk(cx, &view).0.contains("\"plan\""), "its mark, written before it went");
    land(
        cx,
        w,
        &view,
        vec![json!({"role": "user", "content": sent}), json!({"role": "assistant", "content": [{"type": "text", "text": "ok"}]})],
    );
    board(cx, w, &view);
    assert_eq!(desires(cx, &view), 0, "settled");
    let (d, _) = on_disk(cx, &view);
    assert!(!d.contains("\"plan\""), "the settle's deletion, written: {d}");
    heard(cx, w, &view, vec![json!({"ev": "compaction_started", "source": "manual", "disclosure": ""})]);
    let sid = view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].transcript.journal_id().to_string());
    heard(
        cx,
        w,
        &view,
        vec![
            json!({"ev": "compaction_applied", "previous_session_id": sid, "session_id": "j-next", "chains_advanced": [], "msg_count": 8}),
        ],
    );
    let mut compacted = vec![
        json!({"role": "user", "content": "<context-summary>rail work</context-summary>"}),
        json!({"role": "assistant", "content": "Understood."}),
    ];
    compacted.extend(synaps_client::synthetic::rail_history());
    replace(cx, w, &view, compacted);
    board(cx, w, &view);
    assert!(has(cx, w, format!("agent-tomb {}", row("plan"))), "carried: its tombstone");
    let (_, p) = on_disk(cx, &view);
    assert!(p.contains("\"j-next\"") && p.contains("\"soft\":\"plan\""), "the carry, written: {p}");
}

/// R3-P2-2: a write that failed is tried again by a settled history
/// alone, no click or message after it.
#[cfg(unix)]
#[gpui_kit::test]
fn a_failed_write_is_tried_again_by_a_settled_history_alone(cx: &mut TestAppContext) {
    let (w, view) = rail(cx);
    action(cx, w, "plan", "agent-x");
    away(cx, w, &view);
    view.update(cx, |v, cx| assert!(v.enter("go".into(), cx)));
    board(cx, w, &view);
    let path = view.read_with(cx, |v, _| v.wishes.desires.path().expect("a file").to_path_buf());
    // The disk refuses: a folder where the file goes.
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    let sent = "go\n[block plan v1 dismissed]";
    land(
        cx,
        w,
        &view,
        vec![json!({"role": "user", "content": sent}), json!({"role": "assistant", "content": [{"type": "text", "text": "ok"}]})],
    );
    board(cx, w, &view);
    assert_eq!(desires(cx, &view), 0, "settled in memory");
    assert!(path.is_dir(), "not written");
    // The disk back, the next settled history (a typed notice) tries again.
    std::fs::remove_dir(&path).unwrap();
    let typed = "and\n[block cause v1 dismissed]";
    land(
        cx,
        w,
        &view,
        vec![json!({"role": "user", "content": typed}), json!({"role": "assistant", "content": [{"type": "text", "text": "ok"}]})],
    );
    board(cx, w, &view);
    let text = std::fs::read_to_string(&path).expect("written again");
    assert!(!text.contains("\"plan\""), "{text}");
}

/// R3-P2-3: the files' locks outlive every write that can still touch
/// them. A window closed while its write waits on the writer (held), with
/// a change made since behind it: no other desk owns the file until both
/// are written, its last snapshot carrying the change; then one can.
#[gpui_kit::test]
fn a_closed_window_writes_its_last_change_and_holds_the_file_until_then(cx: &mut TestAppContext) {
    let (w, view) = rail(cx);
    let (writer, path) = view.read_with(cx, |v, _| (v.wishes.home.writer.clone(), v.wishes.desires.path().expect("a file").to_path_buf()));
    let hold = writer.hold();
    action(cx, w, "plan", "agent-x");
    action(cx, w, "cause", "agent-x");
    use gpui_kit::AppContext as _;
    let gone = view.downgrade();
    cx.update_window(w, |_, window, _| window.remove_window()).expect("the window");
    drop(view);
    // Its release runs as the app's effects are flushed.
    cx.update(|_| {});
    cx.run_until_parked();
    assert!(gone.upgrade().is_none(), "the window and its stores are gone");
    assert_eq!(crate::blockfiles::own(&path).err(), Some(crate::blockfiles::NotOwned::Held), "its writes out hold the lock");
    drop(hold);
    writer.drain();
    let text = std::fs::read_to_string(&path).expect("written");
    assert!(text.contains("\"plan\"") && text.contains("\"cause\""), "the last snapshot carried the change behind: {text}");
    let free = crate::blockfiles::own_once_free(&path);
    assert!(free.is_ok(), "every write ran: the lock is free ({:?})", free.err());
}

/// R3-P2-4: memory only is a state, said for as long as it lasts: after a
/// dismiss, a show, a move and a settle, and on a session with no rail.
#[cfg(unix)]
#[gpui_kit::test]
fn memory_only_is_said_for_as_long_as_it_lasts(cx: &mut TestAppContext) {
    let (w, view) = rail(cx);
    let dir = crate::theme::test_home::root().join("memory-only");
    std::fs::create_dir_all(&dir).expect("the folder");
    let path = dir.join(crate::desires::FILE);
    let _other = crate::blockfiles::own(&path).expect("the other desk's lock");
    let home = crate::rail_wishes::Home::take(Some(path), Some(dir.join(crate::presentation::FILE)));
    view.update(cx, |v, cx| v.start_wishes(home, cx));
    away(cx, w, &view);
    let says = format!("agent-memory {} · another desk has its files", crate::rail_wishes::MEMORY_ONLY);
    assert!(has(cx, w, says.clone()), "at once");
    action(cx, w, "plan", "agent-x");
    away(cx, w, &view);
    assert!(!has_sec(cx, &view, "plan") && has(cx, w, says.clone()), "after a dismiss");
    let show = drawn(cx, w, "agent-footer 1 dismissed · show".into()).center();
    click(cx, w, show);
    away(cx, w, &view);
    assert!(has_sec(cx, &view, "plan") && has(cx, w, says.clone()), "after a show");
    action(cx, w, "cause", "agent-slot");
    away(cx, w, &view);
    assert!(has(cx, w, says.clone()), "after a move");
    action(cx, w, "plan", "agent-x");
    away(cx, w, &view);
    view.update(cx, |v, cx| assert!(v.enter("go".into(), cx)));
    board(cx, w, &view);
    let sent = outbox(cx, &view).last().cloned().expect("sent");
    land(
        cx,
        w,
        &view,
        vec![json!({"role": "user", "content": sent}), json!({"role": "assistant", "content": [{"type": "text", "text": "ok"}]})],
    );
    board(cx, w, &view);
    assert_eq!(desires(cx, &view), 0, "settled");
    assert!(has(cx, w, says.clone()), "after a settle");
    open_session(cx, w, &view, crate::demo::SID, Place::Current);
    away(cx, w, &view);
    assert!(view.read_with(cx, |v, _| v.rail_layout().is_none_or(|(_, l)| !l.any())), "a session with no rail");
    assert!(has(cx, w, says), "still said");
}

/// R3-P2-5: the footer's `show` acts on the set it drew, never on a count
/// alike: pressed over `1 dismissed` (plan), held while the rail changes
/// to another `1 dismissed` (cause), it does nothing, and says so.
#[gpui_kit::test]
fn the_footers_show_acts_only_on_what_it_drew(cx: &mut TestAppContext) {
    let (w, view) = rail(cx);
    action(cx, w, "plan", "agent-x");
    away(cx, w, &view);
    action(cx, w, "cause", "agent-x");
    away(cx, w, &view);
    go(cx, w, &view);
    ops(cx, w, &view, "toolu_f1", json!([{"op": "remove", "id": "cause"}]));
    away(cx, w, &view);
    let footer = "agent-footer 1 dismissed · show".to_string();
    let at = drawn(cx, w, footer.clone()).center();
    mouse(cx, w, at, "move");
    mouse(cx, w, at, "down");
    let swap = json!([
        {"op": "remove", "id": "plan"},
        {"op": "put", "id": "cause", "kind": "note", "title": "Root cause", "target": "rail", "slot": "upper", "body": {"text": "Back."}}
    ]);
    ops(cx, w, &view, "toolu_f2", swap);
    let at = drawn(cx, w, footer).center();
    mouse(cx, w, at, "up");
    assert_eq!(note(cx, &view), Some((crate::agent_rail_view::FOOTER_ROW, crate::lease::CHANGED)), "another set: nothing");
    away(cx, w, &view);
    assert!(!has_sec(cx, &view, "cause"), "cause stays dismissed");
    assert_eq!(desires(cx, &view), 2);
}

/// R3-P2-5: a tombstone's `u` acts on the incarnation it drew: pressed,
/// held while the agent replaces the block under the same id (its
/// visibility version the same), it does nothing, and the soft dismissal
/// stays the new one's.
#[gpui_kit::test]
fn a_tombstone_pressed_across_a_replacement_does_nothing(cx: &mut TestAppContext) {
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
    let tomb = format!("agent-tomb {}", row("plan"));
    let token = |cx: &mut TestAppContext| {
        view.read_with(cx, |v, _| {
            v.rail_layout().and_then(|(_, l)| match &l.section(&Ref::parse("plan").expect("an id"))?.state {
                crate::agent_rail::State::Tombstone(t) => Some(t.token),
                _ => None,
            })
        })
    };
    let before = token(cx).expect("its tombstone");
    heard(cx, w, &view, vec![json!({"ev": "turn_started", "turn_baseline": 8, "trigger": "user", "user_text": "go on"})]);
    wait(cx, w, &view, 700);
    let at = drawn(cx, w, tomb.clone()).center();
    mouse(cx, w, at, "move");
    mouse(cx, w, at, "down");
    let again = json!([{"op": "replace", "id": "plan", "kind": "plan", "title": "Another plan", "body": {"steps": [{"id": "a", "label": "A", "state": "todo"}]}}]);
    ops(cx, w, &view, "toolu_t1", again);
    mouse(cx, w, at, "up");
    assert_eq!(note(cx, &view), Some((row("plan"), crate::lease::CHANGED)), "another incarnation: nothing");
    away(cx, w, &view);
    assert_eq!(token(cx), Some(before), "the new one's tombstone, its version the same");
    assert!(view.read_with(cx, |v, _| v.wishes.pres.soft("j-next", &cards_spec::block::BlockId::parse("plan").expect("an id"))));
}

/// Tombstones of the test's choosing (D6a's seam): `cause` at the top
/// level, `n1` a member of `findings`.
struct Tombs;

impl crate::agent_rail::RailVisibility for Tombs {
    fn seen(&self, id: &Ref) -> crate::agent_rail::Seen {
        match id.to_string().as_str() {
            "cause" | "n1" => {
                crate::agent_rail::Seen::Tombstone(crate::agent_rail::Tomb { text: crate::rail_wishes::SOFT.into(), token: 3 })
            }
            _ => crate::agent_rail::Seen::Shown,
        }
    }
}

/// R3-P2-6: what an agent op moves under the pointer does nothing at its
/// first click, and says so; the next acts: the footer's `show`, a
/// tombstone, and a group member's tombstone (it moves with its group).
#[gpui_kit::test]
fn the_footer_and_tombstones_an_agent_op_just_moved_do_nothing_once(cx: &mut TestAppContext) {
    // The footer.
    let (w, view) = rail(cx);
    action(cx, w, "plan", "agent-x");
    away(cx, w, &view);
    go(cx, w, &view);
    let arrive = json!([{"op": "put", "id": "extra", "kind": "status", "title": "Extra", "target": "rail", "slot": "upper", "at": {"after": "now"}, "body": {"text": "one more"}}]);
    let footer = "agent-footer 1 dismissed · show".to_string();
    let y = drawn(cx, w, footer.clone()).top();
    ops(cx, w, &view, "toolu_g1", arrive);
    wait(cx, w, &view, 1000);
    assert!(drawn(cx, w, footer.clone()).top() > y, "moved down");
    let at = drawn(cx, w, footer.clone()).center();
    click(cx, w, at);
    assert_eq!(note(cx, &view), Some((crate::agent_rail_view::FOOTER_ROW, crate::lease::MOVED)), "just moved: nothing");
    assert!(has(cx, w, format!("agent-says {} {}", crate::agent_rail_view::FOOTER_ROW, crate::lease::MOVED)), "said there");
    assert!(!has_sec(cx, &view, "plan"));
    click(cx, w, at);
    away(cx, w, &view);
    assert!(has_sec(cx, &view, "plan"), "the next click shows it");
    // A tombstone, and a group member's.
    let (w, view) = rail(cx);
    view.update(cx, |v, cx| {
        v.agent_rail.vis = Some(std::rc::Rc::new(Tombs));
        cx.notify();
    });
    go(cx, w, &view);
    away(cx, w, &view);
    let g = drawn(cx, w, format!("agent-head {}", row("findings"))).center();
    click(cx, w, g);
    away(cx, w, &view);
    let tomb = format!("agent-tomb {}", row("cause"));
    let member = format!("agent-member-tomb {} {} {}", row("findings"), row("n1"), crate::rail_wishes::SOFT);
    let (y0, y1) = (drawn(cx, w, tomb.clone()).top(), drawn(cx, w, member.clone()).top());
    let arrive = json!([{"op": "put", "id": "extra", "kind": "status", "title": "Extra", "target": "rail", "slot": "upper", "at": {"after": "plan"}, "body": {"text": "one more"}}]);
    ops(cx, w, &view, "toolu_g2", arrive);
    wait(cx, w, &view, 1000);
    assert!(drawn(cx, w, tomb.clone()).top() > y0 && drawn(cx, w, member.clone()).top() > y1, "both moved down");
    for (sel, r) in [(tomb, row("cause")), (member, row("n1"))] {
        let at = drawn(cx, w, sel.clone()).center();
        click(cx, w, at);
        assert_eq!(note(cx, &view), Some((r, crate::lease::MOVED)), "{sel}: just moved, nothing");
        assert!(has(cx, w, format!("agent-says {r} {}", crate::lease::MOVED)), "{sel}: said there");
        click(cx, w, at);
        assert_eq!(note(cx, &view), None, "{sel}: the next click acts");
    }
}

/// R3-P2-5: a group member's tombstone pressed while the agent replaces
/// that member (another incarnation under its id) does nothing, and says
/// so; the next click, on the new one, acts.
#[gpui_kit::test]
fn a_member_tombstone_pressed_across_a_replacement_does_nothing(cx: &mut TestAppContext) {
    let (w, view) = rail(cx);
    view.update(cx, |v, cx| {
        v.agent_rail.vis = Some(std::rc::Rc::new(Tombs));
        cx.notify();
    });
    go(cx, w, &view);
    away(cx, w, &view);
    let g = drawn(cx, w, format!("agent-head {}", row("findings"))).center();
    click(cx, w, g);
    away(cx, w, &view);
    let member = format!("agent-member-tomb {} {} {}", row("findings"), row("n1"), crate::rail_wishes::SOFT);
    let at = drawn(cx, w, member.clone()).center();
    mouse(cx, w, at, "move");
    mouse(cx, w, at, "down");
    let again = json!([{"op": "replace", "id": "n1", "kind": "note", "title": "Trace", "body": {"text": "Another trace."}}]);
    ops(cx, w, &view, "toolu_m1", again);
    mouse(cx, w, at, "up");
    assert_eq!(note(cx, &view), Some((row("n1"), crate::lease::CHANGED)), "another incarnation: nothing");
    away(cx, w, &view);
    let at = drawn(cx, w, member).center();
    click(cx, w, at);
    assert_eq!(note(cx, &view), None, "the new one's: it acts");
}

/// R3-P2-6: the footer is a line of the rail's: its coming (the agent
/// puts a block this desk dismissed) waits under the pointer's lease.
#[gpui_kit::test]
fn the_footer_coming_waits_under_the_pointer(cx: &mut TestAppContext) {
    let (w, view) = rail(cx);
    action(cx, w, "plan", "agent-x");
    away(cx, w, &view);
    go(cx, w, &view);
    ops(cx, w, &view, "toolu_c1", json!([{"op": "remove", "id": "plan"}]));
    away(cx, w, &view);
    assert!(!has(cx, w, "agent-footer 1 dismissed · show".into()), "none hidden: no footer");
    let at = drawn(cx, w, format!("agent-head {}", row("cause"))).center();
    mouse(cx, w, at, "move");
    let back = json!([{"op": "put", "id": "plan", "kind": "note", "title": "Plan", "target": "rail", "slot": "upper", "body": {"text": "Again."}}]);
    ops(cx, w, &view, "toolu_c2", back);
    assert!(view.read_with(cx, |v, _| v.model.lives[v.panes.cur.live].board.waiting.is_some()), "the board waits under the lease");
    assert!(!has(cx, w, "agent-footer 1 dismissed · show".into()));
    away(cx, w, &view);
    assert!(has(cx, w, "agent-footer 1 dismissed · show".into()), "the lease over: it comes");
}

/// R3-P2-1: a notice sent before a restart, not settled, rides again with
/// the next message at once (its mark is written already), and that
/// message's echo, the text as sent, confirms it.
#[cfg(unix)]
#[gpui_kit::test]
fn a_resend_after_a_restart_is_confirmed_by_its_echo(cx: &mut TestAppContext) {
    let (w, view) = rail(cx);
    action(cx, w, "plan", "agent-x");
    away(cx, w, &view);
    view.update(cx, |v, cx| assert!(v.enter("one".into(), cx)));
    board(cx, w, &view);
    assert_eq!(outbox(cx, &view), ["one\n[block plan v1 dismissed]"]);
    written(cx, &view);
    // The desk restarts on the same files (the old one's locks let go).
    let (d, p) = view.read_with(cx, |v, _| (v.wishes.desires.path().map(Into::into), v.wishes.pres.path().map(Into::into)));
    view.update(cx, |v, _| v.wishes = crate::rail_wishes::Stores::at_launch());
    let home = crate::rail_wishes::Home::take(d, p);
    view.update(cx, |v, cx| v.start_wishes(home, cx));
    away(cx, w, &view);
    assert!(view.read_with(cx, |v, _| v.wishes.desires.owns_file()));
    view.update(cx, |v, cx| assert!(v.enter("two".into(), cx)));
    let two = "two\n[block plan v1 dismissed]";
    assert_eq!(outbox(cx, &view), [two], "at once: its mark was written before");
    assert_eq!(in_flight(cx, &view), ["one", "two"]);
    heard(cx, w, &view, vec![json!({"ev": "turn_started", "turn_baseline": 6, "trigger": "user", "user_text": two})]);
    assert_eq!(in_flight(cx, &view), ["one"], "its echo, the text as sent, confirmed it");
}
