//! The presentation (blocks.md §3.5's Pin and Slot, §5.2's soft
//! dismissals, §1.8's `blocks.json` row) against B2's fold: pins and slot
//! moves outrank the agent's `move` and `slot`, the order within a slot,
//! groups, compaction, the file's caps, one writer (a second process in
//! memory only), whole snapshots (a removal persists), the sweep.

use cards_spec::block::{Ref, Slot};
use cards_spec::board::Place;
use serde_json::json;

use super::*;
use crate::desires::tests::{Flushed as _, H, J, dir, id, note};

fn r(s: &str) -> Ref {
    Ref::parse(s).unwrap()
}

/// The presentation in `d`'s file, the launch's sweep run, as its owner
/// (as [`crate::desires::tests::desires`]).
fn presentation(d: &std::path::Path) -> Presentation {
    let t = std::time::Instant::now();
    loop {
        let mut p = Presentation::load(Some(d.join(FILE)));
        if p.owner.is_ok() || t.elapsed() > std::time::Duration::from_secs(10) {
            flush(&mut p);
            return p;
        }
        drop(p);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// The file as a restart would read it, while `d`'s owner lives on.
fn reread(d: &std::path::Path) -> Presentation {
    let mut p = Presentation::load(None);
    p.book = crate::blockfiles::peek::<Book>(&d.join(FILE)).0;
    p
}

/// Every queued write run, here (a test's thread stands for the
/// background task).
fn flush(p: &mut Presentation) {
    while let Some(w) = p.next_write() {
        let _ = p.landed(w.run());
    }
}

/// Where `id` draws on this desk, against `h`'s board.
fn at(p: &Presentation, h: &H, id: &str) -> Placed {
    let s = h.state();
    p.placed(J, s.block(&r(id)).expect("live"))
}

/// `p` on `id`, its write landed.
fn pin(p: &mut Presentation, h: &H, id: &str) -> Result<Placed, String> {
    let s = h.state();
    let out = p.pin(J, s.block(&r(id)).expect("live"));
    flush(p);
    out
}

/// `m` on `id`, its write landed.
fn slot(p: &mut Presentation, h: &H, id: &str) -> Result<Placed, String> {
    let s = h.state();
    let out = p.slot(J, s.block(&r(id)).expect("live"));
    flush(p);
    out
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
    assert_eq!(reread(&d).placed(J, h.state().block(&r("plan")).unwrap()).place, Place::Rail(Slot::Upper), "persisted");
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
    flush(&mut p);
    drop(p);
    let p = presentation(&d);
    let s = h.state();
    let plan = s.block(&r("plan")).unwrap();
    assert_eq!(p.placed(J2, plan), Placed { place: Place::Rail(Slot::Upper), by_user: true, agent_moved: false });
    assert!(p.soft(J2, &id("tests")) && !p.soft(J, &id("tests")));
    let mut p = p;
    assert!(p.unsoft(J2, &id("tests")));
    flush(&mut p);
    assert!(!reread(&d).soft(J2, &id("tests")));
    // Soft ones carry on through the next compaction.
    p.compacted(J2, "j3", &[id("notes")]);
    p.compacted("j3", "j4", &[]);
    flush(&mut p);
    assert!(reread(&d).soft("j4", &id("notes")));
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
    flush(&mut p);
    let q = reread(&d);
    assert!(!q.placed(J, h.state().block(&r("plan")).unwrap()).by_user, "the oldest went");
    assert!(soft.iter().all(|i| q.soft(J, i)), "the 64 newer stay");

    let mut desires = crate::desires::tests::desires(&d);
    desires.dismiss_now("c0", &crate::desires::tests::generation(), &id("tests"), &h.vis()).unwrap();
    let s = h.state();
    let b = s.block(&r("b1")).unwrap();
    for c in 0..=CONVERSATIONS_MAX {
        p.pin(&format!("c{c}"), b).unwrap();
    }
    flush(&mut p);
    let q = reread(&d);
    assert!(!q.placed("c0", b).by_user, "the least recently touched went");
    assert!(q.placed("c1", b).by_user && q.placed(&format!("c{CONVERSATIONS_MAX}"), b).by_user);
    drop(desires);
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

/// Shady #106 P2-3: D7's undo puts back exactly what it says, or changes
/// nothing. At the entry cap the entry put back would be the oldest: the
/// book as it was. Over the file's bytes, the least recently touched
/// conversation goes, not this one: put back as it was.
#[test]
fn putting_back_keeps_what_it_puts_back_or_changes_nothing() {
    let d = dir("putback");
    let mut p = presentation(&d);
    let h = H::with(&["plan"]);
    pin(&mut p, &h, "plan").unwrap();
    let was = p.mark(J, &r("plan"));
    let soft: Vec<_> = (0..ENTRIES_MAX).map(|i| id(&format!("s{i}"))).collect();
    p.compacted("before", J, &soft[..ENTRIES_MAX - 1]);
    pin(&mut p, &h, "plan").unwrap();
    assert_eq!(p.mark(J, &r("plan")), Mark::default(), "unpinned");
    p.compacted("before", J, &soft);
    assert_eq!(p.book.conversations[J].entries.len(), ENTRIES_MAX, "the room filled since");
    let book = p.book.clone();
    assert_eq!(p.put_back(J, &r("plan"), &was), Err(NO_ROOM.into()), "it would be the oldest");
    assert_eq!(p.book, book, "nothing changed");

    // The file's bytes: other conversations filling it to within less
    // than J's entry once it is back.
    p.book = Book::default();
    let full = |n: usize| Conversation {
        touched: n as u64,
        seq: ENTRIES_MAX as u64,
        entries: (0..ENTRIES_MAX)
            .map(|i| Kept {
                entry: Entry::Placed {
                    r: r(&format!("card:c{i:06}")),
                    to: UserPlace::Transcript,
                    pinned: true,
                    agent: Some(Place::Transcript(usize::MAX)),
                },
                seq: i as u64,
            })
            .collect(),
    };
    let one = {
        let mut b = Book::default();
        b.conversations.insert("k000".into(), full(0));
        b.text().len() - Book::default().text().len() + 1
    };
    let n = (FILE_BYTES_MAX - 4096) / one;
    for c in 0..n {
        p.book.conversations.insert(format!("k{c:03}"), full(c));
    }
    let entry = full(0).entries;
    for i in 0..2 * ENTRIES_MAX {
        let k = n + i / ENTRIES_MAX;
        let c = p.book.conversations.entry(format!("k{k:03}")).or_insert(Conversation { entries: Vec::new(), ..full(k) });
        c.entries.push(entry[i % ENTRIES_MAX].clone());
        if p.book.text().len() > FILE_BYTES_MAX - 100 {
            break;
        }
    }
    let before = p.book.text().len();
    assert!(before <= FILE_BYTES_MAX && before > FILE_BYTES_MAX - 200, "{before}");
    p.put_back(J, &r("plan"), &was).unwrap();
    assert_eq!(p.mark(J, &r("plan")), was, "put back as it was");
    assert!(p.book.text().len() <= FILE_BYTES_MAX);
    assert!(!p.book.conversations.contains_key("k000"), "the least recently touched went");
}

/// Shady #106 P2-4: a counter behind its entries in the file (an older
/// desk's undo wrote it) is read past them: the next placement sorts last.
#[test]
fn a_counter_behind_its_entries_is_read_past_them() {
    let d = dir("seq");
    let entry = json!({"ref": "plan", "to": "upper", "pin": 1, "agent": "t0", "seq": 3});
    let text = json!({"version": FORMAT, "conversations": {J: {"touched": 1, "seq": 0, "entries": [entry]}}});
    std::fs::write(d.join(FILE), text.to_string()).unwrap();
    let mut p = presentation(&d);
    let h = H::with(&["plan", "b1"]);
    pin(&mut p, &h, "b1").unwrap();
    assert_eq!(p.user_rail(J), [(r("plan"), Slot::Upper), (r("b1"), Slot::Upper)]);
}

/// One writer: a second store of the file (another process, here another
/// in this one) doesn't get its lock, and keeps its placements in memory
/// only: drawn at once, never written, the file untouched; said. Once the
/// owner is gone, the next launch owns it.
#[test]
fn a_second_desk_keeps_its_placements_in_memory_only() {
    let d = dir("two");
    let h = H::with(&["plan"]);
    let s = h.state();
    let b = s.block(&r("plan")).unwrap();
    let mut x = presentation(&d);
    x.pin("jx", b).unwrap();
    flush(&mut x);
    let before = std::fs::read(d.join(FILE)).unwrap();
    let mut y = Presentation::load(Some(d.join(FILE)));
    flush(&mut y);
    assert!(y.trouble.as_deref().is_some_and(|t| t.contains("memory only")), "{:?}", y.trouble);
    assert!(!y.placed("jx", b).by_user, "the file isn't read");
    assert!(y.pin("jy", b).unwrap().by_user && y.placed("jy", b).by_user, "drawn at once");
    assert!(y.next_write().is_none() && !y.writing(), "never written");
    assert_eq!(std::fs::read(d.join(FILE)).unwrap(), before);
    x.pin("jx2", b).unwrap();
    flush(&mut x);
    drop((x, y));
    let z = presentation(&d);
    assert!(z.trouble.is_none() && z.placed("jx", b).by_user && z.placed("jx2", b).by_user && !z.placed("jy", b).by_user);
}

/// A pin draws at once, and its write runs off the UI thread: the call
/// touches no disk. A write that fails is said, memory keeps the change,
/// and the next change's write carries it all; one landing keeps what
/// memory changed after it.
#[test]
fn a_pin_draws_at_once_and_is_written_off_the_ui_thread() {
    let d = dir("p-background");
    let h = H::with(&["plan"]);
    let s = h.state();
    let b = s.block(&r("plan")).unwrap();
    let mut p = presentation(&d);
    // The disk refuses: a folder where the file goes.
    std::fs::create_dir(d.join(FILE)).unwrap();
    let disk = crate::blockfiles::on_this_thread();
    assert_eq!(p.pin(J, b).unwrap().place, Place::Rail(Slot::Upper), "drawn at once");
    assert!(p.writing());
    let w = p.next_write().unwrap();
    assert!(p.next_write().is_none(), "one out at a time");
    assert_eq!(crate::blockfiles::on_this_thread(), disk, "no write on the UI thread");
    let l = std::thread::spawn(move || w.run()).join().unwrap();
    assert!(p.landed(l).is_err() && p.failed.as_deref().is_some_and(|f| f.contains(FILE)), "{:?}", p.failed);
    assert!(p.placed(J, b).by_user && !p.writing(), "memory keeps it, and tries again at the next change");
    std::fs::remove_dir(d.join(FILE)).unwrap();
    p.pin("other", b).unwrap();
    let w = p.next_write().unwrap();
    p.pin("third", b).unwrap();
    assert!(p.landed(w.run()).is_ok() && p.failed.is_none());
    assert!(p.placed("third", b).by_user, "changed after the write: memory's kept");
    assert!(!reread(&d).placed("third", b).by_user && p.writing(), "not in that write: the next");
    flush(&mut p);
    let q = reread(&d);
    assert!(q.placed(J, b).by_user && q.placed("other", b).by_user && q.placed("third", b).by_user, "the next writes carried all");
}

/// `p` on a rail section puts it in the transcript at the call that
/// created it, and there it stays when the agent moves it elsewhere in the
/// transcript; a restart keeps it, and reset placement takes the agent's
/// (P2-6).
#[test]
fn a_transcript_placement_stays_where_the_user_put_it() {
    let d = dir("p-anchor");
    let mut h = H::default();
    let a0 = h.ops(json!([{"op":"put","id":"r","kind":"note","target":"rail","body":{"text":"r"}}]));
    let mut p = presentation(&d);
    assert_eq!(pin(&mut p, &h, "r").unwrap().place, Place::Transcript(a0));
    h.ops(json!([note("other", "x")]));
    h.ops(json!([{"op":"move","id":"r","target":"transcript"}]));
    let moved = h.state().block(&r("r")).unwrap().place;
    assert!(matches!(moved, Place::Transcript(at) if at != a0), "{moved:?}");
    assert_eq!(at(&p, &h, "r"), Placed { place: Place::Transcript(a0), by_user: true, agent_moved: true });
    assert_eq!(at(&reread(&d), &h, "r").place, Place::Transcript(a0), "a restart keeps it");
    assert!(p.reset(J, &r("r")));
    assert_eq!(at(&p, &h, "r"), Placed { place: moved, by_user: false, agent_moved: false }, "reset: the agent's");
}

/// Writes that keep failing keep nothing but memory: no backlog (one
/// write at a time, of memory whole, tried again at the next change), so
/// nothing outgrows the store's caps (P2-5); the disk back, one write
/// carries it all.
#[test]
fn failing_writes_keep_no_backlog() {
    let d = dir("p-failing");
    let h = H::with(&["plan"]);
    let s = h.state();
    let b = s.block(&r("plan")).unwrap();
    let mut p = presentation(&d);
    std::fs::create_dir(d.join(FILE)).unwrap();
    for c in 0..CONVERSATIONS_MAX * 3 {
        p.pin(&format!("c{c}"), b).unwrap();
        let w = p.next_write().expect("one write");
        assert!(p.next_write().is_none());
        assert!(p.landed(w.run()).is_err());
        assert!(!p.writing() && p.next_write().is_none(), "{c}: nothing waits");
    }
    assert!(p.failed.is_some() && p.book.conversations.len() == CONVERSATIONS_MAX);
    assert!(p.placed(&format!("c{}", CONVERSATIONS_MAX * 3 - 1), b).by_user, "memory keeps the newest");
    std::fs::remove_dir(d.join(FILE)).unwrap();
    p.pin("again", b).unwrap();
    flush(&mut p);
    assert!(p.failed.is_none() && reread(&d).book == p.book, "one write, memory whole");
}

/// Removing the last entry persists (Shady R2-P2-4): the only pin unpinned,
/// or its placement reset, the only soft dismissal ended, and one
/// removed while the write adding it is still out. Each stays undone once
/// its write lands, and after a restart (at 22921cf the deletion was never
/// written, and the landing brought the entry back).
#[test]
fn removing_the_last_entry_persists() {
    let h = H::with(&["plan"]);
    let s = h.state();
    let b = s.block(&r("plan")).unwrap();
    for how in ["unpin", "reset", "unsoft", "while-out"] {
        let d = dir(&format!("p-last-{how}"));
        let mut p = presentation(&d);
        match how {
            "unsoft" => p.compacted("before", J, &[id("plan")]),
            _ => {
                p.pin(J, b).unwrap();
            }
        }
        let mut out = p.next_write();
        assert!(out.is_some(), "its write");
        if how != "while-out" {
            assert!(p.landed(out.take().unwrap().run()).is_ok());
            assert!(reread(&d).placed(J, b).by_user || reread(&d).soft(J, &id("plan")), "{how}: written");
        }
        match how {
            "unpin" | "while-out" => assert!(!p.pin(J, b).unwrap().by_user),
            "reset" => assert!(p.reset(J, &r("plan"))),
            _ => assert!(p.unsoft(J, &id("plan"))),
        }
        if let Some(out) = out {
            assert!(p.landed(out.run()).is_ok());
            assert!(!p.placed(J, b).by_user, "the landing of the pin's write brings nothing back");
        }
        flush(&mut p);
        let gone = |p: &Presentation| !p.placed(J, b).by_user && !p.soft(J, &id("plan"));
        assert!(gone(&p) && !p.writing(), "{how}: after its write landed");
        assert!(gone(&reread(&d)) && reread(&d).book.conversations.is_empty(), "{how}: in the file");
        drop(p);
        assert!(gone(&presentation(&d)), "{how}: after a restart");
    }
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

/// A file of `conversations` (key, touched, soft ids with their seqs) as a
/// hand would write it.
fn hand_written(conversations: impl IntoIterator<Item = (String, u64, Vec<(String, u64)>)>) -> String {
    let convs: serde_json::Map<String, serde_json::Value> = conversations
        .into_iter()
        .map(|(k, touched, entries)| {
            let entries: Vec<_> = entries.into_iter().map(|(id, seq)| json!({"soft": id, "seq": seq})).collect();
            (k, json!({"touched": touched, "seq": 0, "entries": entries}))
        })
        .collect();
    json!({"version": 1, "conversations": convs}).to_string()
}

/// A hand-written file past [`FILE_BYTES_MAX`] (no desk writes one) isn't
/// read: it reads as empty, said, and is moved aside (never copied). One
/// at the bound is read; one byte more isn't.
#[test]
fn a_file_past_its_bound_isnt_read() {
    let d = dir("p-oversized");
    let ids = |n: u64| (0..n).map(|i| (format!("s{i:02}"), i + 1)).collect::<Vec<_>>();
    let text = hand_written((0..1000).map(|c| (format!("c{c:03}"), c + 1, ids(ENTRIES_MAX as u64))));
    assert!(text.len() > FILE_BYTES_MAX, "{}", text.len());
    std::fs::write(d.join(FILE), &text).unwrap();
    let p = presentation(&d);
    assert_eq!(p.book.conversations.len(), 0, "not read");
    assert!(p.trouble.as_deref().is_some_and(|t| t.contains(FILE) && t.contains("too big")), "said: {:?}", p.trouble);
    assert!(!d.join(FILE).exists(), "moved aside");
    assert_eq!(std::fs::metadata(d.join(format!("{FILE}.bad"))).unwrap().len(), text.len() as u64, "whole");
    drop(p);

    let one = hand_written([(J.to_string(), 1, ids(1))]);
    for (pad, read) in [(FILE_BYTES_MAX - one.len(), true), (FILE_BYTES_MAX + 1 - one.len(), false)] {
        let d = dir(&format!("p-oversized-{pad}"));
        std::fs::write(d.join(FILE), format!("{one}{}", " ".repeat(pad))).unwrap();
        let p = presentation(&d);
        assert_eq!((p.book.conversations.len(), p.trouble.is_none()), (usize::from(read), read), "{} bytes", one.len() + pad);
    }
}

/// Shady #108 P2-2, the presentation's file too: one to set aside that
/// can't be (every name it may take is a folder) is never written over;
/// its sweep says so, never "set aside"; a pin is kept in memory and
/// draws; neither its write nor the window's last writes the file, nor
/// does a restart's. A change once a name is free sets it aside first,
/// then writes memory whole over it. Past the counts and past the bound.
#[test]
fn a_file_that_cant_be_set_aside_is_never_written_over() {
    let mut h = H::default();
    h.ops(json!([note("plan", "x"), note("tests", "x")]));
    let ids = |n: u64| (0..n).map(|i| (format!("s{i:02}"), i + 1)).collect::<Vec<_>>();
    for (name, text) in [
        ("over", hand_written((0..300u64).map(|c| (format!("c{c:03}"), c + 1, ids(1))))),
        ("too-big", hand_written((0..1000u64).map(|c| (format!("c{c:03}"), c + 1, ids(ENTRIES_MAX as u64))))),
    ] {
        let d = dir(&format!("p-unkept-{name}"));
        let path = d.join(FILE);
        std::fs::write(&path, &text).unwrap();
        let names = crate::desires::tests::aside_names(&path);
        names.iter().for_each(|n| std::fs::create_dir(n).unwrap());
        let mut p = presentation(&d);
        let said = p.trouble.clone().unwrap_or_default();
        pin(&mut p, &h, "plan").unwrap();
        assert_eq!(at(&p, &h, "plan").place, Place::Rail(Slot::Upper), "{name}: kept in memory, it draws");
        let s = h.state();
        let _ = p.pin(J, s.block(&r("tests")).expect("live"));
        if let Some(w) = p.last_write() {
            let _ = w.run();
        }
        assert_eq!(std::fs::read(&path).unwrap(), text.as_bytes(), "{name}: never written over");
        assert!(said.contains("not set aside"), "{name}: {said}");
        drop(p);
        let p = presentation(&d);
        assert!(p.trouble.as_deref().is_some_and(|t| t.contains("not set aside")), "{name}: a restart: {:?}", p.trouble);
        assert_eq!(std::fs::read(&path).unwrap(), text.as_bytes(), "{name}: a restart writes nothing over it");
        assert!(names.iter().all(|n| n.is_dir()), "{name}: no folder touched");
        drop(p);

        let mut p = presentation(&d);
        std::fs::remove_dir(&names[3]).unwrap();
        pin(&mut p, &h, "plan").unwrap();
        assert_eq!(std::fs::read(&names[3]).unwrap(), text.as_bytes(), "{name}: set aside at last");
        let aside = format!("set aside ({FILE}.bad.3)");
        assert!(p.trouble.as_deref().is_some_and(|t| t.contains(&aside)), "{name}: {:?}", p.trouble);
        assert!(reread(&d).book.conversations.contains_key(J), "{name}: then memory, whole, written over it");
    }
}

/// A pipe where the file goes isn't opened (opening one waits for a
/// writer, forever): its read says so at once.
#[cfg(unix)]
#[test]
fn a_pipe_where_the_file_goes_is_never_opened() {
    let d = dir("p-pipe");
    let path = d.join(FILE);
    assert!(std::process::Command::new("mkfifo").arg(&path).status().unwrap().success());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || tx.send(crate::blockfiles::peek::<Book>(&path)).unwrap());
    let (book, why) = rx.recv_timeout(std::time::Duration::from_secs(5)).expect("not waiting on the pipe");
    assert!(book.conversations.is_empty() && why.is_some(), "{why:?}");
}

/// A hand-written file past the counts reads as the writer fits memory:
/// a conversation's oldest entries go past [`ENTRIES_MAX`] (by their
/// order, not the file's), the least recently touched conversations past
/// [`CONVERSATIONS_MAX`]. What's left out is said, and the file is copied
/// aside before anything is written over it.
#[test]
fn a_file_past_the_counts_reads_as_the_writer_fits_it() {
    let d = dir("p-past-the-counts");
    // 100 entries, newest first in the file; and 300 conversations of one.
    let big = (0..100u64).rev().map(|i| (format!("s{i:02}"), i + 1)).collect();
    let mut convs = vec![(J.to_string(), 10_000, big)];
    convs.extend((0..300u64).map(|c| (format!("c{c:03}"), c + 1, vec![("plan".to_string(), 1)])));
    let text = hand_written(convs);
    assert!(text.len() < FILE_BYTES_MAX);
    std::fs::write(d.join(FILE), &text).unwrap();
    let p = presentation(&d);
    let c = &p.book.conversations[J];
    assert_eq!(c.entries.len(), ENTRIES_MAX);
    assert!(c.entries.iter().all(|k| k.seq > 36), "the newest kept");
    assert_eq!(p.book.conversations.len(), CONVERSATIONS_MAX);
    assert!(!p.book.conversations.contains_key("c044") && p.book.conversations.contains_key("c045"), "the most recently touched kept");
    assert!(p.trouble.as_deref().is_some_and(|t| t.contains("81 entries past its caps")), "said: {:?}", p.trouble);
    assert_eq!(std::fs::read_to_string(d.join(format!("{FILE}.bad"))).unwrap(), text, "set aside");
    assert_eq!(std::fs::read_to_string(d.join(FILE)).unwrap(), text, "not written over yet");
    let mut fitted = p.book.clone();
    fitted.fit();
    assert_eq!(fitted, p.book, "within the writer's caps");
    assert_eq!(reread(&d).book, p.book, "a restart reads the same");
}

/// Shady #103 R3-P2-3, the presentation's file too: a write issued holds
/// its lock until it ran, its store dropped or not; the last write carries
/// what memory changed behind the one out.
#[test]
fn a_write_out_holds_the_lock_past_its_store() {
    let d = dir("p-lock-outlives");
    let h = H::with(&["plan", "cause"]);
    let s = h.state();
    let mut x = presentation(&d);
    x.pin("jx", s.block(&r("plan")).unwrap()).unwrap();
    let w = x.next_write().expect("plan's");
    x.pin("jx", s.block(&r("cause")).unwrap()).unwrap();
    assert!(x.next_write().is_none(), "one out: cause behind it");
    let last = x.last_write().expect("memory whole");
    drop(x);
    let y = Presentation::load(Some(d.join(FILE)));
    assert!(!y.owns_file(), "the writes out hold the lock");
    drop(y);
    drop(w.run());
    let y = Presentation::load(Some(d.join(FILE)));
    assert!(!y.owns_file(), "the last write out alone holds it");
    drop(y);
    drop(last.run());
    let mut z = presentation(&d);
    assert!(z.owns_file() && z.placed("jx", s.block(&r("plan")).unwrap()).by_user && z.placed("jx", s.block(&r("cause")).unwrap()).by_user);
    // A write out alone (no last one) holds it too.
    assert!(z.reset("jx", &r("plan")));
    let w = z.next_write().expect("the reset's");
    drop(z);
    let y = Presentation::load(Some(d.join(FILE)));
    assert!(!y.owns_file(), "the write out alone holds the lock");
    drop(y);
    drop(w.run());
    assert!(presentation(&d).owns_file());
}
