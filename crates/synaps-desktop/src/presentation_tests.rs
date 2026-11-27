//! The presentation (blocks.md §3.5's Pin and Slot, §5.2's soft
//! dismissals, §1.8's `blocks.json` row) against B2's fold: pins and slot
//! moves outrank the agent's `move` and `slot`, the order within a slot,
//! groups, compaction, the file's caps, two desks on one file, the sweep.

use cards_spec::block::{Ref, Slot};
use cards_spec::board::Place;
use serde_json::json;

use super::*;
use crate::desires::tests::{H, J, dir, id, note};

fn r(s: &str) -> Ref {
    Ref::parse(s).unwrap()
}

fn presentation(d: &std::path::Path) -> Presentation {
    Presentation::load(Some(d.join(FILE)))
}

/// Where `id` draws on this desk, against `h`'s board.
fn at(p: &Presentation, h: &H, id: &str) -> Placed {
    let s = h.state();
    p.placed(J, s.block(&r(id)).expect("live"))
}

fn pin(p: &mut Presentation, h: &H, id: &str) -> Result<Placed, String> {
    let s = h.state();
    p.pin(J, s.block(&r(id)).expect("live"))
}

fn slot(p: &mut Presentation, h: &H, id: &str) -> Result<Placed, String> {
    let s = h.state();
    p.slot(J, s.block(&r(id)).expect("live"))
}

/// A pin outranks the agent's `move`: the block stays in the upper slot,
/// and its stub says the agent moved it; `p` again takes the agent's
/// placement.
#[test]
fn a_pin_outranks_the_agents_move() {
    let d = dir("pin");
    let mut h = H::default();
    let a0 = h.ops(json!([note("plan", "x")]));
    let mut p = presentation(&d);
    assert_eq!(at(&p, &h, "plan"), Placed { place: Place::Transcript(a0), by_user: false, agent_moved: false });
    assert_eq!(pin(&mut p, &h, "plan").unwrap(), Placed { place: Place::Rail(Slot::Upper), by_user: true, agent_moved: false });
    h.ops(json!([{"op":"move","id":"plan","slot":"end"}]));
    assert_eq!(h.state().block(&r("plan")).unwrap().place, Place::Rail(Slot::End), "the agent's move applied");
    assert_eq!(at(&p, &h, "plan"), Placed { place: Place::Rail(Slot::Upper), by_user: true, agent_moved: true }, "the pin outranks it");
    assert_eq!(p.order(J, &h.state(), Slot::Upper), [r("plan")]);
    assert!(p.order(J, &h.state(), Slot::End).is_empty());
    assert_eq!(presentation(&d).placed(J, h.state().block(&r("plan")).unwrap()).place, Place::Rail(Slot::Upper), "persisted");
    assert_eq!(
        pin(&mut p, &h, "plan").unwrap(),
        Placed { place: Place::Rail(Slot::End), by_user: false, agent_moved: false },
        "unpinned: the agent's"
    );
}

/// A slot move outranks the agent's `slot` and `move` until `m` again,
/// or "reset placement", takes the agent's placement.
#[test]
fn a_slot_move_outranks_the_agents_slot_and_move() {
    let d = dir("slot");
    let mut h = H::default();
    h.ops(json!([{"op":"put","id":"r","kind":"note","target":"rail","body":{"text":"r"}}]));
    let mut p = presentation(&d);
    assert_eq!(slot(&mut p, &h, "r").unwrap(), Placed { place: Place::Rail(Slot::End), by_user: true, agent_moved: false });
    h.ops(json!([{"op":"move","id":"r","target":"transcript"}]));
    assert_eq!(at(&p, &h, "r"), Placed { place: Place::Rail(Slot::End), by_user: true, agent_moved: true });
    h.ops(json!([{"op":"move","id":"r","slot":"upper"}]));
    assert_eq!(at(&p, &h, "r").place, Place::Rail(Slot::End), "still the user's");
    assert_eq!(
        slot(&mut p, &h, "r").unwrap(),
        Placed { place: Place::Rail(Slot::Upper), by_user: false, agent_moved: false },
        "m again: the agent's"
    );
    slot(&mut p, &h, "r").unwrap();
    assert!(p.reset(J, &r("r")));
    assert!(!p.reset(J, &r("r")));
    assert!(!at(&p, &h, "r").by_user, "reset placement: the agent's");
    h.ops(json!([{"op":"move","id":"r","target":"transcript"}]));
    assert_eq!(slot(&mut p, &h, "r"), Err("not in the rail · p brings it there".into()));
}

/// `p` on a rail section brings it into the transcript, at the call that
/// created it; on a pin, `m` moves the pin between the slots, still a pin.
#[test]
fn p_on_a_rail_section_and_m_on_a_pin() {
    let d = dir("p-rail");
    let mut h = H::default();
    let a0 = h.ops(json!([{"op":"put","id":"r","kind":"note","target":"rail","body":{"text":"r"}}, note("plan", "x")]));
    let mut p = presentation(&d);
    assert_eq!(pin(&mut p, &h, "r").unwrap().place, Place::Transcript(a0));
    assert!(slot(&mut p, &h, "r").is_err(), "in the transcript now");
    assert_eq!(pin(&mut p, &h, "r").unwrap().place, Place::Rail(Slot::Upper), "p again: the agent's");
    assert_eq!(pin(&mut p, &h, "plan").unwrap().place, Place::Rail(Slot::Upper));
    assert_eq!(slot(&mut p, &h, "plan").unwrap(), Placed { place: Place::Rail(Slot::End), by_user: true, agent_moved: false });
    assert_eq!(slot(&mut p, &h, "plan").unwrap().place, Place::Rail(Slot::Upper), "still pinned");
    assert_eq!(pin(&mut p, &h, "plan").unwrap(), Placed { place: Place::Transcript(a0), by_user: false, agent_moved: false });
}

/// A group moves with its members: `p` and `m` are off on a member.
#[test]
fn a_group_moves_with_its_members() {
    let d = dir("group");
    let mut h = H::default();
    h.ops(json!([note("a", "1"), note("b", "2")]));
    h.ops(json!([{"op":"merge","into":"g","title":"Research","ids":["a","b"]}]));
    let mut p = presentation(&d);
    assert_eq!(pin(&mut p, &h, "a"), Err("in a group · move the group".into()));
    assert_eq!(pin(&mut p, &h, "g").unwrap().place, Place::Rail(Slot::Upper));
    assert_eq!(slot(&mut p, &h, "b"), Err("in a group · move the group".into()));
    assert_eq!(slot(&mut p, &h, "g").unwrap().place, Place::Rail(Slot::End));
}

/// Within a slot, the user's sections first, in the order the user placed
/// them, then the agent's in its order.
#[test]
fn the_users_sections_come_first_in_a_slot() {
    let d = dir("order");
    let mut h = H::default();
    h.ops(json!([
        {"op":"put","id":"r1","kind":"note","target":"rail","body":{"text":"1"}},
        {"op":"put","id":"r2","kind":"note","target":"rail","body":{"text":"2"}},
        {"op":"put","id":"e1","kind":"note","slot":"end","body":{"text":"e"}},
        note("t1", "t"),
    ]));
    let mut p = presentation(&d);
    assert_eq!(p.order(J, &h.state(), Slot::Upper), [r("r1"), r("r2")]);
    pin(&mut p, &h, "t1").unwrap();
    slot(&mut p, &h, "e1").unwrap();
    assert_eq!(p.order(J, &h.state(), Slot::Upper), [r("t1"), r("e1"), r("r1"), r("r2")]);
    assert!(p.order(J, &h.state(), Slot::End).is_empty());
    slot(&mut p, &h, "r1").unwrap();
    assert_eq!(p.order(J, &h.state(), Slot::End), [r("r1")]);
    h.ops(json!([{"op":"remove","id":"t1"}]));
    assert_eq!(p.order(J, &h.state(), Slot::Upper), [r("e1"), r("r2")], "a removed block draws nowhere");
}

/// A compaction carries pins and slots over by id (the agent's placement
/// then unknown) and the predecessor's dismissals as soft ones; `u` ends
/// one. A restart keeps them.
#[test]
fn compaction_carries_pins_and_soft_dismissals() {
    let d = dir("compaction");
    let h = H::with(&["plan"]);
    let mut p = presentation(&d);
    pin(&mut p, &h, "plan").unwrap();
    const J2: &str = "20261127-020304-beef";
    p.compacted(J, J2, &[id("tests")]);
    p.compacted(J, J2, &[id("tests")]);
    let p = presentation(&d);
    let s = h.state();
    let plan = s.block(&r("plan")).unwrap();
    assert_eq!(p.placed(J2, plan), Placed { place: Place::Rail(Slot::Upper), by_user: true, agent_moved: false });
    assert!(p.soft(J2, &id("tests")) && !p.soft(J, &id("tests")));
    let mut p = p;
    assert!(p.unsoft(J2, &id("tests")));
    assert!(!presentation(&d).soft(J2, &id("tests")));
    // Soft ones carry on through the next compaction.
    p.compacted(J2, "j3", &[id("notes")]);
    p.compacted("j3", "j4", &[]);
    assert!(presentation(&d).soft("j4", &id("notes")));
}

/// The caps: past 64 entries in a conversation the oldest goes; past 256
/// conversations, or 1 MiB, the least recently touched goes. Dropping a
/// conversation leaves its desires alone.
#[test]
fn the_files_caps_drop_the_oldest() {
    let d = dir("caps");
    let mut p = presentation(&d);
    let h = H::with(&["plan", "b1"]);
    pin(&mut p, &h, "plan").unwrap();
    let soft: Vec<_> = (0..ENTRIES_MAX).map(|i| id(&format!("s{i}"))).collect();
    p.compacted("before", J, &soft);
    let q = presentation(&d);
    assert!(!q.placed(J, h.state().block(&r("plan")).unwrap()).by_user, "the oldest went");
    assert!(soft.iter().all(|i| q.soft(J, i)), "the 64 newer stay");

    let mut desires = crate::desires::tests::desires(&d);
    desires.dismiss("c0", &crate::desires::tests::generation(), &id("tests"), &h.vis()).unwrap();
    let s = h.state();
    let b = s.block(&r("b1")).unwrap();
    for c in 0..=CONVERSATIONS_MAX {
        p.pin(&format!("c{c}"), b).unwrap();
    }
    let q = presentation(&d);
    assert!(!q.placed("c0", b).by_user, "the least recently touched went");
    assert!(q.placed("c1", b).by_user && q.placed(&format!("c{CONVERSATIONS_MAX}"), b).by_user);
    assert_eq!(crate::desires::tests::desires(&d).waiting(), [("c0".to_string(), 1)], "its desires stay");

    let mut book = Book::default();
    for c in 0..CONVERSATIONS_MAX {
        let mut conv = Conversation { touched: c as u64, seq: 0, entries: Vec::new() };
        for i in 0..ENTRIES_MAX {
            conv.entries.push(Kept {
                entry: Entry::Placed {
                    r: r(&format!("card:c{i:06}")),
                    to: UserPlace::Transcript,
                    pinned: true,
                    agent: Some(Place::Transcript(usize::MAX)),
                },
                seq: i as u64,
            });
        }
        assert!(conv.bytes() <= CONVERSATION_BYTES_MAX);
        book.conversations.insert(format!("k{c:03}"), conv);
    }
    assert!(book.text().len() > FILE_BYTES_MAX);
    book.fit();
    assert!(book.text().len() <= FILE_BYTES_MAX);
    assert!(!book.conversations.contains_key("k000") && book.conversations.contains_key(&format!("k{:03}", CONVERSATIONS_MAX - 1)));
}

/// Two desks on one file: each write replaces the conversations it
/// changed, and keeps the others'.
#[test]
fn two_desks_on_one_file_keep_each_others_conversations() {
    let d = dir("two");
    let h = H::with(&["plan"]);
    let s = h.state();
    let b = s.block(&r("plan")).unwrap();
    let (mut x, mut y) = (presentation(&d), presentation(&d));
    x.pin("jx", b).unwrap();
    y.pin("jy", b).unwrap();
    x.pin("jx2", b).unwrap();
    let z = presentation(&d);
    assert!(z.placed("jx", b).by_user && z.placed("jy", b).by_user && z.placed("jx2", b).by_user);
}

/// Both files sit next to `settings.json`.
#[test]
fn the_files_are_next_to_the_settings() {
    let settings = crate::theme::settings_path().unwrap();
    assert_eq!(path(), Some(settings.with_file_name(FILE)));
    assert_eq!(crate::desires::path(), Some(settings.with_file_name(crate::desires::FILE)));
}

/// The sweep, and a file that doesn't parse, as for the desires: shared
/// plumbing ([`crate::blockfiles`]).
#[test]
fn a_load_sweeps_and_sets_an_unreadable_file_aside() {
    let d = dir("p-sweep");
    std::fs::write(d.join(format!("{FILE}.tmp.77.1")), "half").unwrap();
    std::fs::write(d.join(FILE), "[]").unwrap();
    let p = presentation(&d);
    assert!(p.trouble.is_some());
    assert!(!d.join(format!("{FILE}.tmp.77.1")).exists());
    assert_eq!(std::fs::read_to_string(d.join(format!("{FILE}.bad"))).unwrap(), "[]");
}
