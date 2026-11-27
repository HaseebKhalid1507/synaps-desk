//! The desires (blocks.md §3.5, §1.8, §5.1, §5.2) against B2's fold: every
//! history here is folded by `cards_spec::board`, so what a desk attaches,
//! settles and draws is checked against what the fold reads. §8.2's D6
//! list for desires, §3.5's notice list, the file (its caps, a full file,
//! a restart, a crash mid-write, the sweep, two processes on one file).

use std::path::{Path, PathBuf};

use cards_spec::block::message::{self, Visibility as Seen};
use cards_spec::block::{BlockId, Generation};
use cards_spec::board::{Board, Event, State, Visibility};
use serde_json::{Value, json};

use super::*;

pub(crate) const J: &str = "20261127-010203-abcd";

pub(crate) fn generation() -> Generation {
    Generation::of(&json!({"role":"user","content":"start"}))
}

pub(crate) fn id(s: &str) -> BlockId {
    BlockId::parse(s).unwrap()
}

/// A fresh folder of the test's own.
pub(crate) fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("synaps-desktop-d6b-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

pub(crate) fn desires(d: &Path) -> Desires {
    Desires::load(Some(d.join(FILE)))
}

pub(crate) fn note(id: &str, text: &str) -> Value {
    json!({"op":"put","id":id,"kind":"note","body":{"text":text}})
}

/// A history, as the board's events: the fold reads it (B2).
#[derive(Clone, Default)]
pub(crate) struct H {
    pub(crate) events: Vec<Event>,
    tools: usize,
}

impl H {
    pub(crate) fn with(ids: &[&str]) -> Self {
        let mut h = H::default();
        h.ops(Value::Array(ids.iter().map(|i| note(i, "x")).collect()));
        h
    }

    /// One assistant message of one block call, bound, its results ended:
    /// its call's position (a transcript anchor).
    pub(crate) fn ops(&mut self, ops: Value) -> usize {
        let at = self.events.len();
        self.tools += 1;
        let t = format!("toolu_{:03}", self.tools);
        let receipt = cards_spec::block::parse_call(&json!({ "ops": ops })).unwrap().receipt();
        self.events.push(Event::Call { tool_id: t.clone(), name: "cards_block".into(), input: json!({ "ops": ops }) });
        self.events.push(Event::Result { tool_id: t, text: receipt.to_string() });
        self.events.push(Event::ResultsEnd);
        at
    }

    pub(crate) fn user(&mut self, text: &str) {
        self.events.push(Event::User { text: text.into() });
    }

    pub(crate) fn state(&self) -> State {
        Board::fold(generation(), &self.events).view(&self.events)
    }

    pub(crate) fn vis(&self) -> Visibility {
        self.state().visibility().clone()
    }
}

/// `n` notices of one kind as typed messages (another desk's, or a TUI
/// user's), 8 ids a line, from version `from`.
fn others(h: &mut H, prefix: &str, n: usize, from: u32, kind: &str) {
    let all: Vec<String> = (0..n).map(|i| format!("{prefix}{i} v{}", from + i as u32)).collect();
    for chunk in all.chunks(8) {
        h.user(&format!("more\n[block {} {kind}]", chunk.join(", ")));
    }
}

fn line(r: &Ride) -> &str {
    r.text.rsplit_once('\n').map_or("", |(_, l)| l)
}

// §8.2's D6 list.

/// A notice rides a typed `Submit` and a `Steer` as their last line, and
/// the fold reads it; never on its own (an empty or blank body), never on a
/// card answer.
#[test]
fn a_notice_rides_a_typed_submit_and_a_steer_never_alone() {
    let d = dir("rides");
    let mut h = H::with(&["tests", "plan"]);
    let (mut a, g) = (desires(&d), generation());
    assert_eq!(a.dismiss(J, &g, &id("tests"), &h.vis()), Ok(Done::Wished(1)));
    assert!(a.dismissed(J, &g, &id("tests"), &h.vis()), "drawn at once, on this desk");
    assert!(!h.vis().dismissed(&id("tests")), "not shared until delivered");
    for (body, how) in [("", Carry::Submit), ("  \n ", Carry::Steer), ("", Carry::HandBack), ("[card c4f9x2a] Yes", Carry::Answer)] {
        let r = a.ride(J, &g, &h.vis(), body, how);
        assert_eq!((r.text.as_str(), r.carrier, r.notices.len()), (body, None, 0), "{how:?} {body:?}: never alone, never on an answer");
    }
    assert_eq!(a.riding(J, &g, &h.vis()), [(id("tests"), Seen::Dismissed, 1)], "still waiting for words");
    let submit = a.ride(J, &g, &h.vis(), "Fix the parser.", Carry::Submit);
    assert_eq!(submit.text, "Fix the parser.\n[block tests v1 dismissed]");
    assert!(submit.carrier.is_some());
    assert!(a.riding(J, &g, &h.vis()).is_empty(), "in flight: on no other message");
    assert_eq!(a.dismiss(J, &g, &id("plan"), &h.vis()), Ok(Done::Wished(2)), "one more than any version seen, its own included");
    let steer = a.ride(J, &g, &h.vis(), "and the lexer", Carry::Steer);
    assert_eq!(steer.text, "and the lexer\n[block plan v2 dismissed]");
    assert_ne!(steer.carrier, submit.carrier);
    h.user(&submit.text);
    h.user(&steer.text);
    let v = h.vis();
    assert!(v.dismissed(&id("tests")) && v.dismissed(&id("plan")), "the fold read both");
    assert_eq!(a.settle(J, &g, &v, &[]), []);
    assert_eq!(a.len(), 0, "settled: dropped");
    assert!(a.dismissed(J, &g, &id("tests"), &v), "drawn from the shared record now");
    assert_eq!(desires(&d).len(), 0, "and dropped from the file");
}

/// A steer that missed its turn's last drain is dropped: the settled
/// history hasn't it, so its desire rides the next message at the same
/// version. A Show after it supersedes it: `shown` at a new version, and
/// the old dismissal never rides again.
#[test]
fn a_dropped_steer_rearms_the_desire_and_a_show_supersedes_it() {
    let h = H::with(&["tests"]);
    let g = generation();
    let mut a = desires(&dir("dropped"));
    a.dismiss(J, &g, &id("tests"), &h.vis()).unwrap();
    let steer = a.ride(J, &g, &h.vis(), "also this", Carry::Steer);
    assert_eq!(line(&steer), "[block tests v1 dismissed]");
    assert_eq!(a.settle(J, &g, &h.vis(), &[steer.carrier.unwrap()]), [], "still queued: still in flight");
    assert!(a.riding(J, &g, &h.vis()).is_empty());
    a.settle(J, &g, &h.vis(), &[]);
    let again = a.ride(J, &g, &h.vis(), "next", Carry::Submit);
    assert_eq!(again.text, "next\n[block tests v1 dismissed]", "re-armed at the same version");

    let mut b = desires(&dir("dropped-show"));
    b.dismiss(J, &g, &id("tests"), &h.vis()).unwrap();
    let steer = b.ride(J, &g, &h.vis(), "also this", Carry::Steer);
    assert_eq!(b.show(J, &g, &id("tests"), &h.vis()), Ok(Done::Wished(2)), "sent: a new version, not a cancel");
    assert!(!b.dismissed(J, &g, &id("tests"), &h.vis()), "drawn shown at once");
    b.settle(J, &g, &h.vis(), &[steer.carrier.unwrap()]);
    assert_eq!(b.riding(J, &g, &h.vis()), [(id("tests"), Seen::Shown, 2)]);
    b.settle(J, &g, &h.vis(), &[]);
    assert_eq!(b.riding(J, &g, &h.vis()), [(id("tests"), Seen::Shown, 2)], "the dropped one is not re-armed: the Show is the desire");
    let next = b.ride(J, &g, &h.vis(), "go", Carry::Submit);
    assert_eq!(next.text, "go\n[block tests v2 shown]");
    let mut h2 = h.clone();
    h2.user(&next.text);
    assert_eq!(b.settle(J, &g, &h2.vis(), &[]), []);
    assert!(!b.dismissed(J, &g, &id("tests"), &h2.vis()) && b.len() == 0);
    // The dropped steer's copy, landing late after all, is stale.
    h2.user(&steer.text);
    assert!(!h2.vis().dismissed(&id("tests")), "v1 under v2: stale");
}

// §3.5's notice list.

/// Dismiss, then Show while the dismissal's carrier is queued: the Show
/// rides the next message, and whichever order the two land in, the later
/// version wins.
#[test]
fn dismiss_and_show_while_a_carrier_is_queued() {
    let h0 = H::with(&["tests"]);
    let g = generation();
    let mut a = desires(&dir("queued"));
    a.dismiss(J, &g, &id("tests"), &h0.vis()).unwrap();
    let queued = a.ride(J, &g, &h0.vis(), "also", Carry::Steer);
    a.show(J, &g, &id("tests"), &h0.vis()).unwrap();
    assert_eq!(a.riding(J, &g, &h0.vis()), [(id("tests"), Seen::Shown, 2)]);
    let mut h = h0.clone();
    h.user(&queued.text);
    assert!(h.vis().dismissed(&id("tests")), "the queued dismissal landed first: shared");
    assert_eq!(a.settle(J, &g, &h.vis(), &[]), []);
    assert!(!a.dismissed(J, &g, &id("tests"), &h.vis()), "this desk draws its own desire meanwhile");
    let show = a.ride(J, &g, &h.vis(), "go on", Carry::Submit);
    assert_eq!(line(&show), "[block tests v2 shown]");
    for order in [[&queued, &show], [&show, &queued]] {
        let mut h = h0.clone();
        for r in order {
            h.user(&r.text);
        }
        assert!(!h.vis().dismissed(&id("tests")), "the later version wins in either order");
        assert_eq!(h.vis().record(&id("tests")).map(|r| r.version), Some(2));
    }
    h.user(&show.text);
    assert_eq!(a.settle(J, &g, &h.vis(), &[]), []);
    assert_eq!(a.len(), 0);
}

/// A carrier lost (a refused send), then Show, then the hand-back's
/// Resend: the Resend carries `shown` at the new version, never the old
/// `dismissed`. A dismissal never sent is simply cancelled by Show instead.
#[test]
fn a_lost_carrier_then_show_then_a_resend_carries_shown_at_the_new_version() {
    let h = H::with(&["tests", "plan"]);
    let g = generation();
    let mut a = desires(&dir("lost"));
    a.dismiss(J, &g, &id("tests"), &h.vis()).unwrap();
    let sent = a.ride(J, &g, &h.vis(), "[block plan edited r1] plan → x\n{\"of\":\"…\"}", Carry::HandBack);
    assert_eq!(line(&sent), "[block tests v1 dismissed]");
    a.lost(sent.carrier.unwrap());
    assert_eq!(a.riding(J, &g, &h.vis()), [(id("tests"), Seen::Dismissed, 1)], "lost: it rides the next message, the same version");
    assert_eq!(a.show(J, &g, &id("tests"), &h.vis()), Ok(Done::Wished(2)), "sent once: a copy may still land, so a new version");
    let resend = a.ride(J, &g, &h.vis(), "[block plan edited r1] plan → x\n{\"of\":\"…\"}", Carry::HandBack);
    assert_eq!(line(&resend), "[block tests v2 shown]");
    assert!(!resend.text.contains("dismissed"));

    let mut b = desires(&dir("lost-cancel"));
    b.dismiss(J, &g, &id("tests"), &h.vis()).unwrap();
    assert_eq!(b.show(J, &g, &id("tests"), &h.vis()), Ok(Done::Cancelled));
    assert_eq!((b.len(), desires(b.path().unwrap().parent().unwrap()).len()), (0, 0), "nothing kept");
    let r = b.ride(J, &g, &h.vis(), "hello", Carry::Submit);
    assert_eq!((r.text.as_str(), r.carrier), ("hello", None), "nothing sent");
    // And the other way: a Show never sent over a delivered dismissal, then
    // dismiss again.
    let mut hd = h.clone();
    hd.user("x\n[block tests v1 dismissed]");
    assert_eq!(b.show(J, &g, &id("tests"), &hd.vis()), Ok(Done::Wished(2)));
    assert_eq!(b.dismiss(J, &g, &id("tests"), &hd.vis()), Ok(Done::Cancelled));
    assert!(b.dismissed(J, &g, &id("tests"), &hd.vis()));
}

/// An old carrier (still queued) lands before the newer wish is sent: it
/// applies on every desk, and this desk draws its own desire, until its
/// next send carries the newer one.
#[test]
fn an_old_carrier_landing_before_the_newer_wish_applies_until_the_next_send() {
    let mut h = H::with(&["tests"]);
    let g = generation();
    let mut a = desires(&dir("old-a"));
    let b = desires(&dir("old-b"));
    a.dismiss(J, &g, &id("tests"), &h.vis()).unwrap();
    let old = a.ride(J, &g, &h.vis(), "also", Carry::Steer);
    a.show(J, &g, &id("tests"), &h.vis()).unwrap();
    h.user(&old.text);
    a.settle(J, &g, &h.vis(), &[]);
    assert!(b.dismissed(J, &g, &id("tests"), &h.vis()), "another desk: the old notice applies");
    assert!(h.vis().dismissed(&id("tests")), "and the agent read it");
    assert!(!a.dismissed(J, &g, &id("tests"), &h.vis()), "this desk: its newer desire");
    let newer = a.ride(J, &g, &h.vis(), "now", Carry::Submit);
    assert_eq!(line(&newer), "[block tests v2 shown]");
    h.user(&newer.text);
    a.settle(J, &g, &h.vis(), &[]);
    assert!(!b.dismissed(J, &g, &id("tests"), &h.vis()) && !a.dismissed(J, &g, &id("tests"), &h.vis()), "every desk agrees again");
    assert_eq!(a.len(), 0);
}

/// Two desks' opposite notices at one version (the other's here typed, as
/// a TUI user or an older desk sends it): the first in history wins, the
/// second is stale, and a desk whose notice lost draws the shared state
/// and says `changed by a later message`.
#[test]
fn two_desks_opposite_notices_at_one_version() {
    let h0 = H::with(&["tests"]);
    let g = generation();
    for theirs_first in [true, false] {
        let mut a = desires(&dir(&format!("opposite-{theirs_first}")));
        a.dismiss(J, &g, &id("tests"), &h0.vis()).unwrap();
        let mine = a.ride(J, &g, &h0.vis(), "mine", Carry::Submit);
        let theirs = "theirs\n[block tests v1 shown]";
        let mut h = h0.clone();
        if theirs_first {
            h.user(theirs);
            h.user(&mine.text);
        } else {
            h.user(&mine.text);
            h.user(theirs);
        }
        let fb = a.settle(J, &g, &h.vis(), &[]);
        assert_eq!(a.len(), 0, "settled either way: v1 is at the effective version");
        if theirs_first {
            assert_eq!(fb, [(id("tests"), Feedback::ChangedByLater)]);
            assert_eq!(a.feedback(J, &id("tests")).map(Feedback::text), Some("changed by a later message"));
            assert!(!a.dismissed(J, &g, &id("tests"), &h.vis()), "the shared state drawn");
        } else {
            assert_eq!(fb, []);
            assert!(a.dismissed(J, &g, &id("tests"), &h.vis()));
        }
        // The next wish clears what was said.
        a.dismiss(J, &g, &id("tests"), &h.vis()).unwrap();
        assert_eq!(a.feedback(J, &id("tests")), None);
    }
}

/// The agent removes the block between the click and the delivery, then
/// puts it again: the absent id is recorded, so the new block stays
/// dismissed, here and on another desk.
#[test]
fn the_agent_removing_the_block_before_delivery_then_putting_it_again() {
    let mut h = H::with(&["tests"]);
    let g = generation();
    let mut a = desires(&dir("removed"));
    let b = desires(&dir("removed-b"));
    a.dismiss(J, &g, &id("tests"), &h.vis()).unwrap();
    h.ops(json!([{"op":"remove","id":"tests"}]));
    let r = a.ride(J, &g, &h.vis(), "ok", Carry::Submit);
    assert_eq!(line(&r), "[block tests v1 dismissed]", "an absent id rides like a live one");
    h.user(&r.text);
    assert_eq!(a.settle(J, &g, &h.vis(), &[]), []);
    h.ops(json!([note("tests", "again")]));
    assert!(h.state().block(&Ref::Block(id("tests"))).is_some(), "put again");
    assert!(a.dismissed(J, &g, &id("tests"), &h.vis()) && b.dismissed(J, &g, &id("tests"), &h.vis()), "still dismissed, every desk");
}

/// One id in both trailers: the desk never builds one (an id's one desire
/// is in one line), and attaches nothing to a body whose own last line
/// would join its trailer (another kind's look-alike, or one naming the
/// same id). A same-kind look-alike, or one that fills no trailer slot,
/// stays the body. A typed message naming an id twice applies neither, so
/// the desire still waits.
#[test]
fn one_id_in_both_trailers() {
    let h = H::with(&["tests"]);
    let g = generation();
    let mut a = desires(&dir("twice"));
    a.dismiss(J, &g, &id("tests"), &h.vis()).unwrap();
    for body in ["see\n[block zz v9 shown]", "see\n[block tests v1 shown]"] {
        let r = a.ride(J, &g, &h.vis(), body, Carry::Submit);
        assert_eq!((r.text.as_str(), r.carrier), (body, None), "{body:?}: its look-alike would join the trailer");
    }
    for body in ["see\n[block zz v9 dismissed]", "[block zz v9 shown]"] {
        let mut b = desires(&dir("twice-body"));
        b.dismiss(J, &g, &id("tests"), &h.vis()).unwrap();
        let r = b.ride(J, &g, &h.vis(), body, Carry::Submit);
        let (read, t) = message::split(&r.text);
        assert_eq!((read, t.text()), (body, "\n[block tests v1 dismissed]".to_string()), "{body:?}");
    }

    let mut h = H::with(&["tests"]);
    let mut a = desires(&dir("twice-many"));
    for i in 0..10 {
        let id = id(&format!("b{i}"));
        if i % 2 == 0 {
            a.dismiss(J, &g, &id, &h.vis()).unwrap();
        } else {
            h.user(&format!("x\n[block b{i} v{} dismissed]", 100 + i));
            a.show(J, &g, &id, &h.vis()).unwrap();
        }
    }
    a.dismiss(J, &g, &id("tests"), &h.vis()).unwrap();
    let body = "see\n[block tests v1 shown]";
    let r = a.ride(J, &g, &h.vis(), body, Carry::Submit);
    let (read, t) = message::split(&r.text);
    assert_eq!(read, body, "two lines of the desk's: the look-alike is the body's");
    assert!(t.named_twice().is_empty() && t.lines.len() == 2 && t.lines[0].visibility == Seen::Dismissed);
    assert_eq!((t.lines[0].notices.len(), t.lines[1].notices.len()), (6, 5));

    let mut b = desires(&dir("twice-typed"));
    let h1 = H::with(&["tests"]);
    b.dismiss(J, &g, &id("tests"), &h1.vis()).unwrap();
    let mut h2 = h1.clone();
    h2.user("hm\n[block tests v1 dismissed]\n[block tests v1 shown]");
    assert_eq!(h2.vis().record(&id("tests")), None, "named twice: neither applies");
    b.settle(J, &g, &h2.vis(), &[]);
    assert_eq!(b.riding(J, &g, &h2.vis()), [(id("tests"), Seen::Dismissed, 1)], "still waiting");
}

/// A restart with a carrier in flight keeps the desire and loses the
/// carrier: against the attached history it is done if the carrier
/// landed, rides again if not, and a second copy landing is stale. A
/// compaction drops the desires, makes the dismissed ids soft in the
/// successor, and the carrier still queued lands there as the new
/// generation's notice.
#[test]
fn a_restart_and_a_compaction_with_a_carrier_in_flight() {
    let h0 = H::with(&["tests"]);
    let g = generation();
    for landed in [false, true] {
        let d = dir(&format!("restart-{landed}"));
        let mut a = desires(&d);
        a.dismiss(J, &g, &id("tests"), &h0.vis()).unwrap();
        let queued = a.ride(J, &g, &h0.vis(), "also", Carry::Steer);
        drop(a);
        let mut a = desires(&d);
        assert_eq!(a.desire(J, &g, &id("tests")).map(|d| (d.dismissed, d.version, d.sent, d.in_flight())), Some((true, 1, true, false)));
        let mut h = h0.clone();
        if landed {
            h.user(&queued.text);
        }
        a.settle(J, &g, &h.vis(), &[]);
        assert!(a.dismissed(J, &g, &id("tests"), &h.vis()));
        if landed {
            assert_eq!(a.len(), 0, "landed while the desk was down: done");
            continue;
        }
        let again = a.ride(J, &g, &h.vis(), "resumed", Carry::Submit);
        assert_eq!(line(&again), "[block tests v1 dismissed]", "rides again at the same version");
        h.user(&again.text);
        h.user(&queued.text);
        let s = h.state();
        assert!(s.log().any(|l| matches!(l.outcome, cards_spec::board::Outcome::Stale { version: 1, later: 1 })), "the second copy: stale");
        a.settle(J, &g, &h.vis(), &[]);
        assert_eq!(a.len(), 0);
    }

    // A compaction with a carrier in flight.
    let d = dir("compacted");
    let mut a = desires(&d);
    let mut p = crate::presentation::Presentation::load(Some(d.join(crate::presentation::FILE)));
    let mut h = H::with(&["tests", "plan"]);
    h.user("x\n[block plan v1 dismissed]");
    a.dismiss(J, &g, &id("tests"), &h.vis()).unwrap();
    let queued = a.ride(J, &g, &h.vis(), "also", Carry::Steer);
    a.show(J, &g, &id("plan"), &h.vis()).unwrap();
    a.dismiss(J, &g, &id("notes"), &h.vis()).unwrap();
    let soft = a.compacted(J, &h.state());
    assert_eq!(soft, [id("notes"), id("tests")], "drawn dismissed here: plan was shown again on this desk");
    assert_eq!((a.len(), desires(&d).len()), (0, 0), "the predecessor's desires dropped");
    const J2: &str = "20261127-020304-beef";
    p.compacted(J, J2, &soft);
    assert!(p.soft(J2, &id("tests")) && p.soft(J2, &id("notes")) && !p.soft(J2, &id("plan")));
    // The successor: a new generation, where the queued steer lands.
    let g2 = Generation::of(&json!({"role":"user","content":"<context-summary>…</context-summary>"}));
    let mut s = H::default();
    s.user(&queued.text);
    s.ops(json!([note("tests", "new")]));
    let shared = Board::fold(g2, &s.events).view(&s.events);
    assert!(shared.visibility().dismissed(&id("tests")), "the carrier's notice counts as the new generation's");
    assert!(a.dismissed(J2, &g2, &id("tests"), shared.visibility()));
    assert!(a.riding(J2, &g2, shared.visibility()).is_empty(), "nothing of the predecessor rides");
}

/// 65 ids' changes push the floor: a notice minted before them, for an id
/// with no record, is stale, and the desk says `not applied · dismiss
/// again`. A retained record at v2 with the floor at 100 still takes a
/// delayed Show at v3: admitted, attached and settled alike.
#[test]
fn sixty_five_ids_and_the_floor() {
    let h0 = H::with(&["tests"]);
    let g = generation();
    let mut a = desires(&dir("floor"));
    a.dismiss(J, &g, &id("tests"), &h0.vis()).unwrap();
    let late = a.ride(J, &g, &h0.vis(), "also", Carry::Steer);
    let mut h = h0.clone();
    h.ops(json!([{"op":"remove","id":"tests"}]));
    others(&mut h, "b", 65, 2, "shown");
    assert_eq!(h.vis().floor(), 2, "one record evicted: the oldest shown");
    h.user(&late.text);
    assert_eq!(h.vis().record(&id("tests")), None, "v1 under the floor: stale");
    assert_eq!(a.settle(J, &g, &h.vis(), &[]), [(id("tests"), Feedback::NotApplied)]);
    assert_eq!(a.feedback(J, &id("tests")).map(Feedback::text), Some("not applied · dismiss again"));
    assert_eq!(a.dismiss(J, &g, &id("tests"), &h.vis()), Ok(Done::Wished(67)), "above everything seen");
    let r = a.ride(J, &g, &h.vis(), "again", Carry::Submit);
    h.user(&r.text);
    assert!(h.vis().dismissed(&id("tests")));

    // Retained at v2, the floor at 100, a delayed Show at v3.
    let mut h = H::with(&["tests"]);
    h.user("x\n[block zz v1 shown]\n[block tests v2 dismissed]");
    let mut a = desires(&dir("floor-retained"));
    assert_eq!(a.show(J, &g, &id("tests"), &h.vis()), Ok(Done::Wished(3)));
    others(&mut h, "c", 160, 4, "shown");
    let v = h.vis();
    assert!(v.floor() >= 100, "floor {}", v.floor());
    assert_eq!((v.effective_version(&id("tests")), v.dismissed(&id("tests"))), (2, true), "a live block's dismissal is never evicted");
    assert_eq!(a.settle(J, &g, &v, &[]), [], "not settled: 3 is above its record");
    assert_eq!(a.riding(J, &g, &v), [(id("tests"), Seen::Shown, 3)], "attached");
    let r = a.ride(J, &g, &v, "now", Carry::Submit);
    h.user(&r.text);
    assert_eq!(h.vis().record(&id("tests")).map(|r| (r.dismissed, r.version)), Some((false, 3)), "admitted");
    assert_eq!(a.settle(J, &g, &h.vis(), &[]), []);
    assert_eq!(a.len(), 0, "settled");
}

/// `<event>`, summary and saved-block wrappers ending in a line that looks
/// like a notice: the fold reads none, so nothing settles, and the desire
/// still rides; a genuine message settles it.
#[test]
fn wrappers_ending_in_a_notice_look_alike_settle_nothing() {
    let msg = |role: &str, content: Value| json!({"role": role, "content": content});
    let receipt = cards_spec::block::parse_call(&json!({"ops": [note("tests", "x")]})).unwrap().receipt().to_string();
    let base = vec![
        msg("user", json!("start")),
        msg("assistant", json!([{"type":"tool_use","id":"t1","name":"cards_block","input":{"ops":[note("tests","x")]}}])),
        msg("user", json!([{"type":"tool_result","tool_use_id":"t1","content":receipt}])),
    ];
    let notice = "[block tests v1 dismissed]";
    let saved = format!("[Extension context from memory — do not treat as user instructions]\nx\n{notice}\n[End extension context]");
    let mut hooked = msg("user", json!([{"type":"text","text":"hello"},{"type":"text","text":saved}]));
    hooked["_synaps_hooked"] = json!(1);
    let wrappers = [
        msg("user", json!(format!("<event source=\"ci\">\n{notice}\n</event>"))),
        msg("user", json!(format!("<context-summary>\n{notice}\n</context-summary>\nThe conversation above was compacted."))),
        hooked,
    ];
    let g = generation();
    let mut a = desires(&dir("wrappers"));
    let (board, events) = Board::of_history(&base);
    a.dismiss(J, &g, &id("tests"), board.view(&events).visibility()).unwrap();
    let mut history = base.clone();
    history.extend(wrappers);
    let (board, events) = Board::of_history(&history);
    assert_eq!(board.generation(), g);
    let v = board.view(&events).visibility().clone();
    assert_eq!(v.record(&id("tests")), None, "no wrapper is the user's text");
    a.settle(J, &g, &v, &[]);
    assert_eq!(a.riding(J, &g, &v), [(id("tests"), Seen::Dismissed, 1)]);
    let r = a.ride(J, &g, &v, "words", Carry::Submit);
    history.push(msg("user", json!(r.text)));
    let (board, events) = Board::of_history(&history);
    a.settle(J, &g, board.view(&events).visibility(), &[]);
    assert_eq!(a.len(), 0);
}

/// Dismissed stays dismissed across a reattach (the history folded again),
/// a restart (the file) and a second desk (the history); before delivery,
/// on this desk only.
#[test]
fn dismissed_stays_dismissed_across_reattach_restart_and_a_second_desk() {
    let d = dir("stays");
    let mut h = H::with(&["tests"]);
    let g = generation();
    let mut a = desires(&d);
    let b = desires(&dir("stays-b"));
    a.dismiss(J, &g, &id("tests"), &h.vis()).unwrap();
    let reattached = H { events: h.events.clone(), tools: 0 };
    a.settle(J, &g, &reattached.vis(), &[]);
    assert!(a.dismissed(J, &g, &id("tests"), &reattached.vis()), "reattach, undelivered: the desire");
    assert!(desires(&d).dismissed(J, &g, &id("tests"), &h.vis()), "restart, undelivered: the file");
    assert!(!b.dismissed(J, &g, &id("tests"), &h.vis()), "another desk: not yet");
    let r = a.ride(J, &g, &h.vis(), "go", Carry::Submit);
    h.user(&r.text);
    h.ops(json!([{"op":"patch","id":"tests","set":{"text":"y"}}]));
    a.settle(J, &g, &h.vis(), &[]);
    let fresh = Board::fold(g, &h.events).view(&h.events);
    for (what, desk) in [("this desk", &a), ("restarted", &desires(&d)), ("another desk", &b)] {
        assert!(desk.dismissed(J, &g, &id("tests"), fresh.visibility()), "{what}");
    }
}

// The file.

/// A full desire file refuses the next wish before it draws, in the
/// conversation (64) and across the desk (512); a restart loses no desire.
#[test]
fn a_full_desire_file_refuses_before_drawing_and_a_restart_loses_no_desire() {
    let d = dir("full");
    let h = H::default();
    let g = generation();
    let mut a = desires(&d);
    for c in 0..PER_DESK / PER_CONVERSATION {
        for i in 0..PER_CONVERSATION {
            let conv = format!("{J}-{c}");
            assert!(matches!(a.dismiss(&conv, &g, &id(&format!("b{i}")), &h.vis()), Ok(Done::Wished(_))), "{conv} b{i}");
        }
    }
    assert_eq!(a.len(), PER_DESK);
    let one_more = id("one-more");
    assert_eq!(a.dismiss(&format!("{J}-0"), &g, &one_more, &h.vis()), Err(Refused(FULL_HERE.into())));
    assert_eq!(a.dismiss("another", &g, &one_more, &h.vis()), Err(Refused(FULL_DESK.into())));
    assert!(!a.dismissed("another", &g, &one_more, &h.vis()), "refused before it drew");
    // A wish over one kept is no new desire: it goes in.
    assert!(matches!(a.show(&format!("{J}-3"), &g, &id("b5"), &h.vis()), Ok(Done::Cancelled)));
    assert!(matches!(a.dismiss(&format!("{J}-3"), &g, &id("b5"), &h.vis()), Ok(Done::Wished(_))));
    let before: Vec<_> = (0..8)
        .flat_map(|c| (0..64).map(move |i| (c, i)))
        .map(|(c, i)| a.desire(&format!("{J}-{c}"), &g, &id(&format!("b{i}"))).cloned())
        .collect();
    let text = std::fs::read_to_string(d.join(FILE)).unwrap();
    assert!(text.len() <= RESERVE, "{} bytes", text.len());
    let b = desires(&d);
    assert_eq!(b.len(), PER_DESK, "a restart loses no desire");
    let after: Vec<_> = (0..8)
        .flat_map(|c| (0..64).map(move |i| (c, i)))
        .map(|(c, i)| b.desire(&format!("{J}-{c}"), &g, &id(&format!("b{i}"))).cloned())
        .collect();
    assert_eq!(before, after);
}

/// The reserve: long journals bind it before 512 desires do; past it the
/// wish is refused before it draws, and the file stays in its 64 KiB. An
/// entry at its largest is within 64 bytes.
#[test]
fn the_reserved_64_kib_refuses_before_drawing() {
    let d = dir("reserve");
    let h = H::default();
    let g = generation();
    let mut a = desires(&d);
    let big = "z".repeat(BLOCK_ID_LEN);
    let mut refused = None;
    for c in 0..PER_DESK {
        let conv = format!("{c:04}{}", "j".repeat(KEY_MAX - 4));
        match a.dismiss(&conv, &g, &id(&big), &h.vis()) {
            Ok(_) => {}
            Err(Refused(why)) => {
                assert!(!a.dismissed(&conv, &g, &id(&big), &h.vis()));
                refused = Some((c, why));
                break;
            }
        }
    }
    let (c, why) = refused.expect("the reserve binds first");
    assert!(why.starts_with(FULL_DESK) && c < PER_DESK, "{c}: {why}");
    assert!(std::fs::metadata(d.join(FILE)).unwrap().len() as usize <= RESERVE);
    assert_eq!(desires(&d).len(), c, "every accepted one kept");
    let mut m = serde_json::Map::new();
    m.insert(big.clone(), json!({"d": 1, "v": cards_spec::block::message::VERSION_MAX, "s": 1}));
    let entry = Value::Object(m).to_string();
    assert!(entry.len() - 2 <= ENTRY_MAX, "{entry}: {} bytes", entry.len() - 2);
}

const BLOCK_ID_LEN: usize = 24;

/// Accepted means written: a wish whose write fails (the folder can't be
/// made, the lock is held past its wait) is refused, nothing draws, and a
/// restart has nothing of it.
#[test]
fn a_failed_write_refuses_the_wish() {
    let h = H::with(&["tests"]);
    let g = generation();
    let d = dir("unwritable");
    std::fs::write(d.join("not-a-folder"), "x").unwrap();
    let mut a = Desires::load(Some(d.join("not-a-folder").join(FILE)));
    let why = a.dismiss(J, &g, &id("tests"), &h.vis()).unwrap_err();
    assert!(why.0.contains("not-a-folder"), "{why:?}");
    assert!(!a.dismissed(J, &g, &id("tests"), &h.vis()));

    let d = dir("locked");
    let mut a = desires(&d);
    let held = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(d.join(format!("{FILE}.lock"))).unwrap();
    held.try_lock().unwrap();
    let t = std::time::Instant::now();
    let why = a.dismiss(J, &g, &id("tests"), &h.vis()).unwrap_err();
    assert!(why.0.contains("not locked") && t.elapsed() >= crate::blockfiles::LOCK_WAIT, "{why:?}");
    assert!(!a.dismissed(J, &g, &id("tests"), &h.vis()));
    held.unlock().unwrap();
    assert_eq!(desires(&d).len(), 0);
    assert_eq!(a.dismiss(J, &g, &id("tests"), &h.vis()), Ok(Done::Wished(1)), "the lock let go: written");
}

/// No config folder: nothing can be kept, so every wish is refused.
#[test]
fn without_a_folder_every_wish_is_refused() {
    let h = H::with(&["tests"]);
    let mut a = Desires::load(None);
    assert!(a.dismiss(J, &generation(), &id("tests"), &h.vis()).is_err());
    assert!(a.dismiss("", &generation(), &id("tests"), &h.vis()).is_err());
}

/// A file that doesn't parse is set aside (`.bad`) and read as empty; an
/// entry that doesn't is left out, the rest kept; the next write goes on.
#[test]
fn an_unreadable_file_is_set_aside() {
    let d = dir("unreadable");
    std::fs::write(d.join(FILE), "{not json").unwrap();
    let mut a = desires(&d);
    assert!(a.trouble.is_some() && a.len() == 0);
    assert_eq!(std::fs::read_to_string(d.join(format!("{FILE}.bad"))).unwrap(), "{not json");
    a.dismiss(J, &generation(), &id("tests"), &H::default().vis()).unwrap();
    let g = generation_key(&generation());
    let text = json!({"version": 1, "conversations": {J: {"generation": g, "desires": {
        "tests": {"d": 1, "v": 1, "s": 0}, "Bad": {"d": 1, "v": 1, "s": 0}, "plan": {"d": 1, "v": 0, "s": 0}}}}});
    std::fs::write(d.join(FILE), text.to_string()).unwrap();
    assert_eq!(desires(&d).len(), 1);
}

/// What crashes left beside the file is swept at a load: regular files of
/// exactly `<file>.tmp.<pid>.<n>`, never a link (nor what it names), a
/// folder or another name.
#[test]
fn a_load_sweeps_what_crashes_left() {
    let d = dir("sweep");
    let leftover = d.join(format!("{FILE}.tmp.4242.7"));
    std::fs::write(&leftover, "half").unwrap();
    std::fs::create_dir(d.join(format!("{FILE}.tmp.4242.8"))).unwrap();
    std::fs::write(d.join(format!("{FILE}.tmp.x.9")), "other").unwrap();
    std::fs::write(d.join(format!("{FILE}.lock")), "").unwrap();
    let outside = dir("sweep-outside").join("kept");
    std::fs::write(&outside, "kept").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, d.join(format!("{FILE}.tmp.4242.9"))).unwrap();
    let a = desires(&d);
    assert!(a.trouble.is_none());
    assert!(!leftover.exists());
    assert!(d.join(format!("{FILE}.tmp.4242.8")).is_dir() && d.join(format!("{FILE}.tmp.x.9")).exists());
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "kept");
    #[cfg(unix)]
    assert!(d.join(format!("{FILE}.tmp.4242.9")).symlink_metadata().is_ok(), "the link stays");
}

/// The file is this user's only.
#[cfg(unix)]
#[test]
fn the_file_is_this_users_only() {
    use std::os::unix::fs::PermissionsExt as _;
    let d = dir("mode");
    let mut a = desires(&d);
    a.dismiss(J, &generation(), &id("tests"), &H::default().vis()).unwrap();
    assert_eq!(std::fs::metadata(d.join(FILE)).unwrap().permissions().mode() & 0o777, 0o600);
}

/// Another generation's desires under the same journal (its history
/// replaced) count nowhere: dropped at the settle, and a wish there starts
/// the conversation over.
#[test]
fn another_generation_under_the_journal_drops_its_desires() {
    let h = H::with(&["tests"]);
    let (g, g2) = (generation(), Generation::of(&json!({"role":"user","content":"other"})));
    let d = dir("generation");
    let mut a = desires(&d);
    a.dismiss(J, &g, &id("tests"), &h.vis()).unwrap();
    assert!(a.desire(J, &g2, &id("tests")).is_none() && !a.dismissed(J, &g2, &id("tests"), &h.vis()));
    a.settle(J, &g2, &h.vis(), &[]);
    assert_eq!((a.len(), desires(&d).len()), (0, 0));
    a.dismiss(J, &g, &id("tests"), &h.vis()).unwrap();
    a.dismiss(J, &g2, &id("plan"), &h.vis()).unwrap();
    assert_eq!(desires(&d).waiting(), [(J.to_string(), 1)], "the new generation's only");
}

/// The chip's `×` keeps a notice back from the next message only; past 8
/// of a kind, the rest ride the next message, the oldest versions first.
#[test]
fn the_chip_keeps_one_back_and_eight_ride_at_most() {
    let h = H::default();
    let g = generation();
    let mut a = desires(&dir("chip"));
    for i in 0..11 {
        a.dismiss(J, &g, &id(&format!("b{i:02}")), &h.vis()).unwrap();
    }
    assert!(a.hold(J, &g, &id("b00"), &h.vis()));
    assert!(!a.hold(J, &g, &id("zz"), &h.vis()));
    let first = a.ride(J, &g, &h.vis(), "one", Carry::Submit);
    assert_eq!(line(&first), "[block b01 v2, b02 v3, b03 v4, b04 v5, b05 v6, b06 v7, b07 v8, b08 v9 dismissed]");
    let second = a.ride(J, &g, &h.vis(), "two", Carry::Submit);
    assert_eq!(line(&second), "[block b00 v1, b09 v10, b10 v11 dismissed]", "the held one rides the message after");
}

// Two desks on one file: processes.

/// The child side of the tests below, run by them as a process of its own
/// (this test binary, this test, `SYNAPS_D6B_CHILD` set); a no-op otherwise.
#[test]
fn child() {
    let Ok(task) = std::env::var("SYNAPS_D6B_CHILD") else { return };
    let (what, folder) = task.split_once(':').unwrap();
    let d = PathBuf::from(folder);
    let h = H::default();
    match what {
        "merge" => {
            // The parent holds the file's lock mid-write: this desk's load
            // and wish wait for it, then read what it wrote.
            while !d.join("holding").exists() {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            std::fs::write(d.join("waiting"), "").unwrap();
            let mut a = desires(&d);
            assert_eq!(a.dismiss("child", &generation(), &id("c0"), &h.vis()), Ok(Done::Wished(1)));
        }
        "crash" => {
            let changes = [Change::Wish {
                conv: "child".into(),
                generation: generation_key(&generation()),
                id: id("crashed"),
                dismissed: true,
                version: 1,
            }];
            let _ = blockfiles::update_then::<Book>(
                &d.join(FILE),
                |b| apply(b, &changes, Some("child")),
                || -> std::io::Result<()> { std::process::abort() },
            );
            unreachable!("aborted between the temporary file and the rename");
        }
        _ => panic!("{what}"),
    }
}

fn spawn(task: &str, d: &Path) -> std::process::Child {
    std::process::Command::new(std::env::current_exe().unwrap())
        .args(["desires::tests::child", "--exact", "--test-threads=1", "--nocapture"])
        .env("SYNAPS_D6B_CHILD", format!("{task}:{}", d.display()))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap()
}

/// Two desk processes on one desire file: while one is mid-write (its
/// temporary file written, not yet renamed), the other's load and wish
/// wait for the file's lock, then merge into what it wrote, so neither
/// loses the other's desire. (Unlocked, the second would read the old
/// file, and one of the two writes would be lost.)
#[test]
fn a_second_process_waits_for_a_write_and_merges_into_it() {
    let d = dir("processes");
    let child = spawn("merge", &d);
    let mine =
        [Change::Wish { conv: "parent".into(), generation: generation_key(&generation()), id: id("p0"), dismissed: true, version: 1 }];
    let held = blockfiles::update_then::<Book>(
        &d.join(FILE),
        |b| apply(b, &mine, Some("parent")),
        || {
            std::fs::write(d.join("holding"), "")?;
            let t = std::time::Instant::now();
            while !d.join("waiting").exists() {
                if t.elapsed() > std::time::Duration::from_secs(60) {
                    return Err(std::io::Error::other("no second process"));
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            // Its load is at the lock by now (it waits up to 250 ms).
            std::thread::sleep(std::time::Duration::from_millis(100));
            Ok(())
        },
    );
    assert!(held.is_ok(), "{held:?}");
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(desires(&d).waiting(), [("child".to_string(), 1), ("parent".to_string(), 1)]);
}

/// A desk killed between writing its temporary file and the rename: the
/// file is as it was, and the next load sweeps the leftover.
#[test]
fn a_crash_mid_write_keeps_the_file_and_the_next_load_sweeps() {
    let d = dir("crash");
    let mut a = desires(&d);
    a.dismiss(J, &generation(), &id("tests"), &H::default().vis()).unwrap();
    let before = std::fs::read(d.join(FILE)).unwrap();
    assert!(!spawn("crash", &d).wait_with_output().unwrap().status.success());
    assert_eq!(std::fs::read(d.join(FILE)).unwrap(), before, "atomic: the old file whole");
    let left = || {
        std::fs::read_dir(&d)
            .unwrap()
            .filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().starts_with(&format!("{FILE}.tmp.")))
            .count()
    };
    assert_eq!(left(), 1, "the dead writer's temporary file");
    let b = desires(&d);
    assert_eq!((left(), b.waiting()), (0, vec![(J.to_string(), 1)]));
}

/// Another process's wish of the other kind for the same id, at the same
/// version or later, is under this click: re-minted above it, the newest
/// click wins on every process that shares the file.
#[test]
fn a_wish_over_another_processs_later_one_is_minted_above_it() {
    let d = dir("remint");
    let h = H::with(&["tests"]);
    let g = generation();
    let (mut a, mut b) = (desires(&d), desires(&d));
    a.dismiss(J, &g, &id("tests"), &h.vis()).unwrap();
    a.ride(J, &g, &h.vis(), "go", Carry::Submit);
    assert_eq!(a.show(J, &g, &id("tests"), &h.vis()), Ok(Done::Wished(2)));
    assert_eq!(b.dismiss(J, &g, &id("tests"), &h.vis()), Ok(Done::Wished(3)), "b had seen none of it");
    assert!(b.dismissed(J, &g, &id("tests"), &h.vis()), "drawn as clicked");
    a.dismiss(J, &g, &id("plan"), &h.vis()).unwrap();
    assert_eq!(
        a.desire(J, &g, &id("tests")).map(|d| (d.dismissed, d.version, d.sent)),
        Some((true, 3, false)),
        "a takes it at its next write"
    );
    assert_eq!(b.dismiss(J, &g, &id("tests"), &h.vis()), Ok(Done::Unchanged));
}

/// Two desks in one process (two windows' stores) on one file: a desk's
/// settle drops only what it settled, and the other's wishes survive it.
#[test]
fn a_second_desk_on_the_file_keeps_the_others_wishes() {
    let d = dir("second");
    let mut h = H::with(&["tests", "plan"]);
    let g = generation();
    let (mut a, mut b) = (desires(&d), desires(&d));
    a.dismiss(J, &g, &id("tests"), &h.vis()).unwrap();
    b.dismiss(J, &g, &id("plan"), &h.vis()).unwrap();
    assert_eq!(b.len(), 2, "the write took the file as it was");
    let r = a.ride(J, &g, &h.vis(), "go", Carry::Submit);
    assert_eq!(line(&r), "[block tests v1 dismissed]", "a's desire, a's carrier");
    h.user(&r.text);
    a.settle(J, &g, &h.vis(), &[]);
    let c = desires(&d);
    assert_eq!(c.desire(J, &g, &id("plan")).map(|d| d.version), Some(1), "b minted from what b had seen");
    assert!(c.desire(J, &g, &id("tests")).is_none());
}
