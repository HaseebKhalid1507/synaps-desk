//! The rail's layout, without a window (blocks.md §4's D6 tests that are
//! the layout's alone): slot order, the cap across both slots, the fifth
//! expansion, a collapse and a placement reset freeing a place, groups in
//! one slot, the seam, overflow per kind, and NOW's order (its label on
//! the earliest-started call while it runs). The window's are
//! `agent_rail_tests.rs`.

use cards_spec::block::Slot;
use serde_json::{Value, json};
use synaps_client::blocks::{Drawn, Ref, Worker};
use synaps_client::protocol::{DaemonFrame, SessionEvent, decode_event, decode_frame};
use synaps_client::synthetic::{block_input, block_receipt, rail_history};
use synaps_client::transcript::Transcript;

use super::*;

/// History messages: a user's, then each round of ops as one block call
/// and its receipt.
fn history(rounds: &[Value]) -> Vec<Value> {
    let mut msgs = vec![json!({"role": "user", "content": "go"})];
    for (k, ops) in rounds.iter().enumerate() {
        let id = format!("toolu_t{k}");
        msgs.push(json!({"role": "assistant", "content": [{"type": "tool_use", "id": id, "name": "cards_block", "input": block_input(ops.clone())}]}));
        msgs.push(json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": id, "content": block_receipt(ops)}]}));
    }
    msgs
}

fn attach(msgs: &[Value]) -> Transcript {
    let a = json!({"type": "attached", "client": 2,
        "meta": {"id": "s1", "name": "n", "title": "t", "model": "m", "cwd": "/", "lifecycle": "live", "busy": false, "awaiting_input": 0},
        "view": {"model": "m", "thinking_level": "low", "reasoning_level": "low", "context_window": 200000},
        "conversation": {"header": {}, "messages_len": msgs.len(), "api_messages": msgs, "cost": 0.0},
        "streaming": false, "pending_prompts": [], "client_input": "shared", "input_owner": 2});
    let Ok(DaemonFrame::Attached(a)) = decode_frame(&a.to_string()) else { panic!("an attach") };
    let mut t = Transcript::new();
    t.load_attached(&a).expect("attached");
    t
}

/// The board a history draws.
fn drawn_of(msgs: &[Value]) -> Drawn {
    let t = attach(msgs);
    Worker::default().run(&t.blocks_input().expect("blocks"), &mut || true).expect("folded")
}

fn drawn(rounds: &[Value]) -> Drawn {
    drawn_of(&history(rounds))
}

fn note(id: &str, slot: &str) -> Value {
    json!({"op": "put", "id": id, "kind": "note", "title": id, "target": "rail", "slot": slot, "body": {"text": format!("{id}'s text")}})
}

fn r(id: &str) -> Ref {
    Ref::parse(id).expect("an id")
}

/// A note's lines as laid out, standing in for the window's text system
/// (40 characters a line): the layout's rules take the count as given.
/// The window's tests lay the text out (`agent_rail_tests.rs`).
fn est(text: &str) -> usize {
    crate::blocks::note_lines(text, 40.)
}

/// The names in each slot as drawn, each with its state.
fn drawn_names(l: &Layout) -> [Vec<String>; 2] {
    l.slots.clone().map(|s| {
        s.iter()
            .map(|x| {
                let st = match &x.state {
                    State::Open => "open",
                    State::Summary => "summary",
                    State::Over => "over",
                    State::Listed => "listed",
                    State::Tombstone(_) => "tomb",
                };
                format!("{}:{st}", x.s.block.id)
            })
            .collect()
    })
}

/// Three sections in each slot, every one open by the agent's default.
fn six() -> Drawn {
    drawn(&[json!([
        note("u1", "upper"),
        note("u2", "upper"),
        note("u3", "upper"),
        note("e1", "end"),
        note("e2", "end"),
        note("e3", "end")
    ])])
}

/// A click on section `id`'s chevron as drawn now.
fn click(d: &Drawn, local: &mut Local, id: &str) {
    let l = layout(d, &AllShown, local);
    local.toggle(&l, &r(id));
}

#[test]
fn the_cap_is_four_across_both_slots_in_drawing_order() {
    let d = six();
    let l = layout(&d, &AllShown, &Local::default());
    assert_eq!(drawn_names(&l), [vec!["u1:open", "u2:open", "u3:open"], vec!["e1:open", "e2:over", "e3:over"]]);
    assert_eq!(l.more[0], None, "the upper slot keeps all its own");
    let m = l.more[1].as_ref().expect("the end slot's leftovers");
    assert_eq!(more_text(m), "+2 more · e2, e3");
    assert_eq!(l.sections, 6, "nothing is ever dropped");
    // Its click lists their headers.
    let local = Local { listed: [false, true], ..Local::default() };
    let l = layout(&d, &AllShown, &local);
    assert_eq!(drawn_names(&l)[1], ["e1:open", "e2:listed", "e3:listed"]);
}

#[test]
fn a_slot_draws_in_placement_order_and_at_after_places_within_it() {
    let d = drawn(&[
        json!([note("a", "upper"), note("b", "end"), note("c", "upper")]),
        json!([{"op": "put", "id": "d", "kind": "note", "target": "rail", "slot": "upper", "at": {"after": "a"}, "body": {"text": "d"}}]),
        json!([{"op": "move", "id": "b", "slot": "upper"}]),
    ]);
    let l = layout(&d, &AllShown, &Local::default());
    assert_eq!(drawn_names(&l), [vec!["a:open", "d:open", "c:open", "b:open"], vec![]]);
}

#[test]
fn a_user_expansion_takes_a_place_first_and_the_last_agent_one_gives_way() {
    let d = six();
    let mut local = Local { listed: [false, true], ..Local::default() };
    click(&d, &mut local, "e3");
    let l = layout(&d, &AllShown, &local);
    // e3 first; the agent's three that remain in drawing order: u1, u2, u3.
    assert_eq!(drawn_names(&l), [vec!["u1:open", "u2:open", "u3:open"], vec!["e1:listed", "e2:listed", "e3:open"]]);
    assert_eq!(l.more[1].as_ref().map(more_text).as_deref(), Some("+2 more · e1, e2"));
}

#[test]
fn a_fifth_user_expansion_folds_the_users_oldest_in_either_slot() {
    for (fifth, folded) in [("e3", "u1"), ("u2", "u1")] {
        let d = six();
        let mut local = Local { listed: [true, true], ..Local::default() };
        // Four expansions spread across both slots, in this order. A
        // section the agent opened is folded by a click first, then
        // expanded by the user.
        for id in ["u1", "e1", "u3", "e2"] {
            let open = layout(&d, &AllShown, &local).section(&r(id)).is_some_and(|s| s.state == State::Open);
            if open {
                click(&d, &mut local, id);
            }
            click(&d, &mut local, id);
        }
        let l = layout(&d, &AllShown, &local);
        assert_eq!(l.open(), ["u1", "u3", "e1", "e2"], "the four places are the user's");
        if fifth == "u2" {
            // u2 is the agent's, folded into its slot's `+N`: listed.
            assert_eq!(l.section(&r("u2")).map(|s| s.state.clone()), Some(State::Listed));
        }
        click(&d, &mut local, fifth);
        let l = layout(&d, &AllShown, &local);
        let mut want: Vec<String> =
            ["u1", "u2", "u3", "e1", "e2", "e3"].iter().filter(|x| ["e1", "u3", "e2", fifth].contains(x)).map(|x| x.to_string()).collect();
        want.sort_by_key(|x| ["u1", "u2", "u3", "e1", "e2", "e3"].iter().position(|y| y == x));
        assert_eq!(l.open(), want, "the newest click wins; {folded} folds ({fifth} fifth)");
        assert_eq!(local.expanded(), [r("e1"), r("u3"), r("e2"), r(fifth)], "oldest first: {folded} went");
        // Back to the agent's default: it expands again when a place frees.
        click(&d, &mut local, "e2");
        let l = layout(&d, &AllShown, &local);
        assert!(l.open().contains(&folded.to_string()), "{folded} takes the freed place: {:?}", l.open());
    }
}

#[test]
fn a_collapse_frees_a_place() {
    let d = six();
    let mut local = Local::default();
    click(&d, &mut local, "u2");
    let l = layout(&d, &AllShown, &local);
    assert_eq!(drawn_names(&l), [vec!["u1:open", "u2:summary", "u3:open"], vec!["e1:open", "e2:open", "e3:over"]]);
    assert_eq!(summary(&l.section(&r("u2")).expect("u2").s), "u2's text");
}

/// D6b's placements, for the seam's tests.
struct Placed(Vec<(Ref, Slot)>, Vec<Ref>, usize);

impl RailVisibility for Placed {
    fn seen(&self, id: &Ref) -> Seen {
        if self.1.contains(id) { Seen::Hidden } else { Seen::Shown }
    }

    fn placed(&self) -> &[(Ref, Slot)] {
        &self.0
    }

    fn dismissed(&self) -> usize {
        self.2
    }
}

#[test]
fn the_users_placements_come_first_and_a_reset_frees_their_place() {
    let d = six();
    // The user moved e3 into the upper slot, then pinned the transcript's
    // block `t` there: both first, in the order the user made them.
    let t = drawn(&[
        json!([note("u1", "upper"), note("u2", "upper"), note("u3", "upper"), note("e1", "end"), note("e2", "end"), note("e3", "end"),
            {"op": "put", "id": "t", "kind": "note", "body": {"text": "in the transcript"}}]),
    ]);
    let vis = Placed(vec![(r("e3"), Slot::Upper), (r("t"), Slot::Upper), (r("gone"), Slot::End)], vec![], 0);
    let l = layout(&t, &vis, &Local::default());
    assert_eq!(drawn_names(&l), [vec!["e3:open", "t:open", "u1:open", "u2:open", "u3:over"], vec!["e1:over", "e2:over"]]);
    assert!(l.section(&r("e3")).is_some_and(|s| s.user && s.slot == Slot::Upper));
    // Reset placement: e3 back at the end, the last; its place goes to u3.
    let l = layout(&d, &AllShown, &Local::default());
    assert_eq!(drawn_names(&l), [vec!["u1:open", "u2:open", "u3:open"], vec!["e1:open", "e2:over", "e3:over"]]);
}

#[test]
fn hidden_sections_go_and_tombstones_take_no_place() {
    let d = six();
    let vis = Placed(vec![], vec![r("u1")], 1);
    let l = layout(&d, &vis, &Local::default());
    assert_eq!(drawn_names(&l), [vec!["u2:open", "u3:open"], vec!["e1:open", "e2:open", "e3:over"]]);
    assert_eq!((l.sections, l.dismissed), (5, 1), "N sections · M dismissed");
    struct Soft;
    impl RailVisibility for Soft {
        fn seen(&self, id: &Ref) -> Seen {
            if id.to_string() == "u2" {
                Seen::Tombstone(Tomb { text: "dismissed before compaction · u show".into(), token: 1 })
            } else {
                Seen::Shown
            }
        }
    }
    let l = layout(&d, &Soft, &Local::default());
    assert_eq!(drawn_names(&l), [vec!["u1:open", "u2:tomb", "u3:open"], vec!["e1:open", "e2:open", "e3:over"]]);
    assert_eq!(l.sections, 5, "a tombstone is not a section");
}

#[test]
fn a_group_sits_in_one_slot_with_its_members_folded_until_opened() {
    let d = drawn(&[json!([
        note("u1", "upper"),
        note("n1", "end"),
        note("n2", "end"),
        {"op": "merge", "into": "g", "title": "Findings", "ids": ["n1", "n2"]}
    ])]);
    let l = layout(&d, &AllShown, &Local::default());
    assert_eq!(drawn_names(&l), [vec!["u1:open"], vec!["g:summary"]], "created folded: no place");
    let g = l.section(&r("g")).expect("the group");
    assert_eq!(g.s.members.iter().map(|m| m.block.id.to_string()).collect::<Vec<_>>(), ["n1", "n2"]);
    assert_eq!(summary(&g.s), "2 blocks");
    let mut local = Local::default();
    click(&d, &mut local, "g");
    let l = layout(&d, &AllShown, &local);
    assert_eq!(drawn_names(&l), [vec!["u1:open"], vec!["g:open"]]);
    assert_eq!(lines(&l.section(&r("g")).expect("g").s, &State::Open, &local, &est, None), 2, "a line a member");
}

#[test]
fn the_users_fold_wins_until_the_agents_next_collapse() {
    let mut rounds = vec![json!([note("a", "upper")])];
    let d = drawn(&rounds);
    let mut local = Local::default();
    click(&d, &mut local, "a");
    assert_eq!(drawn_names(&layout(&d, &AllShown, &local))[0], ["a:summary"]);
    rounds.push(json!([{"op": "collapse", "id": "a", "open": true}]));
    let d = drawn(&rounds);
    assert_eq!(drawn_names(&layout(&d, &AllShown, &local))[0], ["a:open"], "the agent's collapse op wins");
}

#[test]
fn a_new_generation_or_a_block_gone_drops_the_local_state() {
    let d = six();
    let mut local = Local::default();
    local.of(&d, &HashSet::new());
    click(&d, &mut local, "u1");
    local.more.insert(r("u2"));
    let live: HashSet<Ref> = ["u1", "u3"].map(r).into_iter().collect();
    local.of(&d, &live);
    assert_eq!((local.choice(&layout(&d, &AllShown, &local).section(&r("u1")).expect("u1").s).is_some(), local.more.len()), (true, 0));
    let other = drawn_of(&{
        let mut h = history(&[json!([note("u1", "upper")])]);
        h[0] = json!({"role": "user", "content": "another conversation"});
        h
    });
    local.of(&other, &live);
    assert!(local.expanded().is_empty() && local.choice(&other.rail[0][0]).is_none(), "another generation starts afresh");
}

#[test]
fn overflow_per_kind_keeps_to_twelve_lines() {
    let d = drawn_of(&rail_history());
    let local = Local::default();
    let l = layout(&d, &AllShown, &local);
    let sec = |id: &str| l.section(&r(id)).expect(id).s.clone();
    // A 20-step plan: 11 rows around the first step not done (s07, two
    // above it), and `+9 more · 4 done` (s01..s04 done, above the window).
    let Body::Plan(pl) = &sec("plan").block.body else { panic!("a plan") };
    assert_eq!(plan_window(&pl.steps, None), (4, 11, Some((9, 4))));
    assert_eq!(plan_window(&pl.steps, Some(9)), (9, 11, Some((9, 6))), "the wheel moved it; all six done are above");
    assert_eq!(plan_window(&pl.steps, Some(30)), (9, 11, Some((9, 6))), "never past its end");
    assert_eq!(lines(&sec("plan"), &State::Open, &local, &est, None), 12);
    // A 12-step plan shows whole.
    assert_eq!(plan_window(&pl.steps[..12], None), (0, 12, None));
    assert_eq!(lines(&sec("now"), &State::Open, &local, &est, None), 2, "status");
    assert_eq!(lines(&sec("rot"), &State::Open, &local, &est, None), 3, "metric, with its series");
    assert_eq!(lines(&sec("file"), &State::Open, &local, &est, None), 1, "file");
    // A diff of 3 hunks (14 lines with their `@@`): 10, `+4 lines`, its foot.
    assert_eq!(
        diff_lines(
            match &sec("retry").block.body {
                Body::Diff(x) => x,
                _ => panic!("a diff"),
            },
            None
        ),
        (14, true)
    );
    assert_eq!(diff_shown(14, true), (10, true, 12));
    assert_eq!(lines(&sec("retry"), &State::Open, &local, &est, None), 12);
    // A long note: 11 lines and `more`; opened, whole.
    let (n, cut) = match &sec("cause").block.body {
        Body::Note(x) => note_shown(est(&x.text), false),
        _ => panic!("a note"),
    };
    assert_eq!((n, cut), (NOTE_LINES, true));
    assert_eq!(lines(&sec("cause"), &State::Open, &local, &est, None), 12);
    let mut opened = Local::default();
    opened.more.insert(r("cause"));
    assert!(lines(&sec("cause"), &State::Open, &opened, &est, None) > 12, "`more` is the user's own resize");
    // Folded: the summary line; leftovers: nothing.
    assert_eq!(lines(&sec("plan"), &State::Summary, &local, &est, None), 1);
    assert_eq!(summary(&sec("plan")), "6/20 done");
    assert_eq!(summary(&sec("rot")), "1.0 · was 2.0");
    assert_eq!(summary(&sec("now")), "Running the regression test");
    for id in ["plan", "now", "cause", "file", "rot", "retry", "findings"] {
        assert!(lines(&sec(id), &State::Open, &local, &est, None) <= LINES || id == "never", "{id} keeps to {LINES}");
    }
}

#[test]
fn the_rail_history_lays_out_both_slots() {
    let d = drawn_of(&rail_history());
    let l = layout(&d, &AllShown, &Local::default());
    assert_eq!(
        drawn_names(&l),
        [vec!["plan:open", "now:open", "cause:open", "file:open"], vec!["rot:over", "retry:over", "findings:summary"]]
    );
    assert_eq!(l.more[1].as_ref().map(more_text).as_deref(), Some("+2 more · rot, retry"));
}

// -- NOW --------------------------------------------------------------------

fn ev(v: Value) -> SessionEvent {
    decode_event(&v).unwrap_or_else(|e| panic!("{e}: {v}"))
}

fn llm(v: Value) -> SessionEvent {
    ev(json!({"ev": "stream", "event": v}))
}

/// A live turn's transcript, then `steps` applied: each `(call, "use" |
/// "queued" | "started" | "result")`.
fn turn(t: &mut Transcript, steps: &[(&str, &str)]) {
    for (id, what) in steps {
        if *what == "use" {
            t.apply(&llm(json!({"kind": "llm", "llm": "tool_use_start", "tool_name": "read", "tool_id": id})));
        }
        t.apply(&match *what {
            "use" => llm(
                json!({"kind": "llm", "llm": "tool_use", "tool_name": "read", "tool_id": id, "input": {"path": format!("src/{id}.rs")}}),
            ),
            "queued" => llm(json!({"kind": "llm", "llm": "tool_queued", "tool_id": id})),
            "started" => llm(json!({"kind": "llm", "llm": "tool_started", "tool_id": id})),
            _ => llm(json!({"kind": "llm", "llm": "tool_result", "tool_id": id, "result": "ok"})),
        });
    }
}

fn now(t: &Transcript, prompt: Option<&str>, card: bool) -> String {
    // A one-field input is kept bare: a read's is its path.
    let call = |c: &synaps_client::transcript::ToolCall| (c.name.clone(), c.input.clone());
    now_of(prompt.map(str::to_string), card, t.busy(), t.items(), &call).text()
}

#[test]
fn now_takes_the_first_that_applies_in_ctrl_js_order() {
    let mut t = attach(&history(&[]));
    assert_eq!(now(&t, None, false), "idle");
    t.apply(&ev(json!({"ev": "turn_started", "turn_baseline": 1, "trigger": "user", "user_text": "go"})));
    assert_eq!(now(&t, None, false), "thinking");
    t.apply(&llm(json!({"kind": "llm", "llm": "response_start"})));
    t.apply(&llm(json!({"kind": "llm", "llm": "text", "text": "Reading"})));
    assert_eq!(now(&t, None, false), "writing");
    turn(&mut t, &[("a", "use"), ("b", "use"), ("c", "use"), ("a", "started"), ("b", "started"), ("c", "queued")]);
    assert_eq!(now(&t, None, false), "read src/a.rs  +1 running · 1 queued");
    assert_eq!(now(&t, None, true), "waiting for your answer", "a card the agent waits on comes before");
    assert_eq!(now(&t, Some("allow bash"), true), "waiting for you · allow bash", "a prompt first");
}

#[test]
fn nows_label_stays_on_the_earliest_started_call_while_it_runs() {
    let mut t = attach(&history(&[]));
    t.apply(&ev(json!({"ev": "turn_started", "turn_baseline": 1, "trigger": "user", "user_text": "go"})));
    t.apply(&llm(json!({"kind": "llm", "llm": "response_start"})));
    // Parallel reads, started out of their call order: b first.
    turn(&mut t, &[("a", "use"), ("b", "use"), ("c", "use"), ("b", "started")]);
    assert_eq!(now(&t, None, false), "read src/b.rs");
    let mut seen = Vec::new();
    for step in [("a", "started"), ("c", "started"), ("a", "result"), ("d", "use"), ("d", "started"), ("c", "result")] {
        turn(&mut t, &[step]);
        seen.push(now(&t, None, false));
    }
    assert!(seen.iter().all(|s| s.starts_with("read src/b.rs")), "no flicker while b runs: {seen:?}");
    turn(&mut t, &[("b", "result")]);
    assert_eq!(now(&t, None, false), "read src/d.rs", "then the earliest-started still running");
}

/// D6b's soft dismissal of a group's member (Shady #104 P2-5): the member
/// keeps its place in the group as its tombstone; a hidden one leaves the
/// list; the structure says which are which, and an agent change to a
/// tombstone's content changes nothing drawn.
#[test]
fn a_members_tombstone_keeps_its_place_in_the_group_as_a_tombstone() {
    let ops = |text: &str| {
        json!([
            note("m1", "upper"),
            {"op": "put", "id": "m2", "kind": "note", "title": "m2", "target": "rail", "slot": "upper", "body": {"text": text}},
            note("m3", "upper"),
            {"op": "merge", "into": "g", "title": "Group", "ids": ["m1", "m2", "m3"]}
        ])
    };
    struct Soft;
    impl RailVisibility for Soft {
        fn seen(&self, id: &Ref) -> Seen {
            match id.to_string().as_str() {
                "m2" => Seen::Tombstone(Tomb { text: "dismissed before compaction · u show".into(), token: 3 }),
                "m3" => Seen::Hidden,
                _ => Seen::Shown,
            }
        }
    }
    let d = drawn(&[ops("m2's text")]);
    let l = layout(&d, &Soft, &Local::default());
    let g = l.section(&r("g")).expect("g");
    let ids: Vec<String> = g.s.members.iter().map(|m| m.block.id.to_string()).collect();
    assert_eq!(ids, ["m1", "m2"], "m3 hidden: out of the list");
    assert_eq!(g.tombs, vec![None, Some(Tomb { text: "dismissed before compaction · u show".into(), token: 3 })]);
    assert_eq!(lines(&g.s, &State::Open, &Local::default(), &est, None), 2, "a tombstone is one line");
    // Its structure: a member shown or a tombstone differs.
    let shown = layout(&d, &AllShown, &Local::default());
    let sh = |l: &Layout| shape(l, &Local::default(), &est, &|_| None);
    let hide3 = {
        struct Hide;
        impl RailVisibility for Hide {
            fn seen(&self, id: &Ref) -> Seen {
                if id.to_string() == "m3" { Seen::Hidden } else { Seen::Shown }
            }
        }
        layout(&d, &Hide, &Local::default())
    };
    assert_ne!(sh(&l), sh(&hide3), "a tombstone's place in the structure");
    assert_ne!(sh(&shown), sh(&hide3));
    // The agent's change under the tombstone: the same structure.
    let d2 = drawn(&[ops("m2's text"), json!([{"op": "patch", "id": "m2", "set": {"text": "changed"}}])]);
    assert_eq!(sh(&layout(&d2, &Soft, &Local::default())), sh(&l));
}

/// The structure the rail's lease compares (Shady #104 P2-2, D5's rule): a
/// note open and whole is as tall as its text lays out, so its text is in
/// it (a patch of the same length can wrap longer); a note cut is its
/// fixed lines whatever its text.
#[test]
fn a_whole_notes_text_is_in_the_structure_a_cut_ones_is_not() {
    let put = |id: &str, text: &str| json!({"op": "put", "id": id, "kind": "note", "title": id, "target": "rail", "slot": "upper", "body": {"text": text}});
    let long = "x".repeat(40 * 14);
    let d = |a: &str, b: &str| {
        drawn(&[json!([put("short", a), put("long", &long)]), json!([{"op": "patch", "id": "long", "set": {"text": b}}])])
    };
    let sh = |d: &Drawn| shape(&layout(d, &AllShown, &Local::default()), &Local::default(), &est, &|_| None);
    let other = "y".repeat(40 * 14);
    assert_ne!(sh(&d("aaaa bbbb", &long)), sh(&d("aaaabbbb", &long)), "whole: its text (a space out: the same lines)");
    assert_eq!(sh(&d("aaaa bbbb", &long)), sh(&d("aaaa bbbb", &other)), "cut: its fixed lines");
}

/// A tombstone's token is in the structure the rail's lease compares
/// (Shady #104 R2-3), a section's own as a group member's: another token
/// (D6b's visibility version for its action) is another structure, the
/// same token and text the same one.
#[test]
fn a_tombstones_token_is_in_the_structure_its_own_and_a_members() {
    /// `id` softly dismissed, its tombstone at `token`.
    struct Tok(&'static str, u64);
    impl RailVisibility for Tok {
        fn seen(&self, id: &Ref) -> Seen {
            if id.to_string() == self.0 {
                Seen::Tombstone(Tomb { text: "dismissed before compaction · u show".into(), token: self.1 })
            } else {
                Seen::Shown
            }
        }
    }
    let d = drawn(&[json!([
        note("t1", "upper"),
        note("m1", "upper"),
        note("m2", "upper"),
        {"op": "merge", "into": "g", "title": "Group", "ids": ["m1", "m2"]}
    ])]);
    let sh = |v: &Tok| shape(&layout(&d, v, &Local::default()), &Local::default(), &est, &|_| None);
    // A section's own tombstone.
    assert!(matches!(layout(&d, &Tok("t1", 7), &Local::default()).section(&r("t1")).map(|s| &s.state), Some(State::Tombstone(_))));
    assert_eq!(sh(&Tok("t1", 7)), sh(&Tok("t1", 7)), "the same token: the same structure");
    assert_ne!(sh(&Tok("t1", 7)), sh(&Tok("t1", 8)), "another token, its own: another structure");
    // A member's.
    assert_eq!(sh(&Tok("m2", 7)), sh(&Tok("m2", 7)));
    assert_ne!(sh(&Tok("m2", 7)), sh(&Tok("m2", 8)), "another token, a member's: another structure");
}
