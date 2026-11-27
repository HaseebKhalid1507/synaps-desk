//! The desires (blocks.md §3.5, §1.8, §5.1, §5.2) against B2's fold: every
//! history here is folded by `cards_spec::board`, so what a desk attaches,
//! settles and draws is checked against what the fold reads. §8.2's D6
//! list for desires, §3.5's notice list, the file (its caps, a full file,
//! a restart, a crash mid-write, the sweep, two processes on one file).

use std::path::{Path, PathBuf};

use cards_spec::block::message::{self, Visibility as Seen};
use cards_spec::block::{BlockId, Generation, Ref};
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

/// The desires in `d`'s file, the launch's sweep run.
pub(crate) fn desires(d: &Path) -> Desires {
    let mut a = Desires::load(Some(d.join(FILE)));
    flush(&mut a);
    a
}

/// Every queued write run, here (a test's thread stands for the
/// background task): what landed.
pub(crate) fn flush(a: &mut Desires) -> Vec<Landed> {
    let mut out = Vec::new();
    while let Some(w) = a.next_write() {
        out.extend(a.landed(w.run()));
    }
    out
}

/// What a wish did once its write landed (or at once).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Now {
    Unchanged,
    Cancelled,
    Wished(u32),
}

/// The calls whose writes the tests want landed before they look: each
/// runs its writes here ([`flush`]).
pub(crate) trait Flushed {
    fn dismiss_now(&mut self, conv: &str, g: &Generation, id: &BlockId, shared: &Visibility) -> Result<Now, Refused>;
    fn show_now(&mut self, conv: &str, g: &Generation, id: &BlockId, shared: &Visibility) -> Result<Now, Refused>;
    fn settle_now(&mut self, conv: &str, g: &Generation, shared: &Visibility, in_flight: &[u64]) -> Vec<(BlockId, Feedback)>;
    fn compacted_now(&mut self, conv: &str, state: &State) -> Vec<BlockId>;
    fn ride_now(&mut self, conv: &str, g: &Generation, shared: &Visibility, body: &str, how: Carry) -> Ride;
}

fn landed(a: &mut Desires, asked: Result<Ask, Refused>, conv: &str, id: &BlockId) -> Result<Now, Refused> {
    match asked? {
        Ask::Unchanged => Ok(Now::Unchanged),
        Ask::Saving => {
            let done = flush(a)
                .into_iter()
                .rev()
                .find_map(|l| match l {
                    Landed::Wish { conv: c, id: i, done } if c == conv && i == *id => Some(done),
                    _ => None,
                })
                .expect("its write landed");
            done.map(|d| match d {
                Done::Unchanged => Now::Unchanged,
                Done::Cancelled => Now::Cancelled,
                Done::Wished(v) => Now::Wished(v),
            })
        }
    }
}

/// What a wish's landing said: its outcome (one wish landed).
fn wish_done(landed: &[Landed]) -> Vec<Result<Done, Refused>> {
    landed
        .iter()
        .filter_map(|l| match l {
            Landed::Wish { done, .. } => Some(done.clone()),
            Landed::Send { .. } => None,
        })
        .collect()
}

impl Flushed for Desires {
    fn dismiss_now(&mut self, conv: &str, g: &Generation, id: &BlockId, shared: &Visibility) -> Result<Now, Refused> {
        let asked = self.dismiss(conv, g, id, shared);
        landed(self, asked, conv, id)
    }

    fn show_now(&mut self, conv: &str, g: &Generation, id: &BlockId, shared: &Visibility) -> Result<Now, Refused> {
        let asked = self.show(conv, g, id, shared);
        landed(self, asked, conv, id)
    }

    fn settle_now(&mut self, conv: &str, g: &Generation, shared: &Visibility, in_flight: &[u64]) -> Vec<(BlockId, Feedback)> {
        let out = self.settle(conv, g, shared, in_flight);
        flush(self);
        out
    }

    fn compacted_now(&mut self, conv: &str, state: &State) -> Vec<BlockId> {
        let out = self.compacted(conv, state);
        flush(self);
        out
    }

    fn ride_now(&mut self, conv: &str, g: &Generation, shared: &Visibility, body: &str, how: Carry) -> Ride {
        match self.ride(conv, g, shared, body, how) {
            Riding::Go(r) => r,
            Riding::Wait(t) => flush(self)
                .into_iter()
                .find_map(|l| match l {
                    Landed::Send { ticket, ride } if ticket == t => Some(ride),
                    _ => None,
                })
                .expect("its mark landed"),
        }
    }
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
    assert_eq!(a.dismiss_now(J, &g, &id("tests"), &h.vis()), Ok(Now::Wished(1)));
    assert!(a.dismissed(J, &g, &id("tests"), &h.vis()), "drawn at once, on this desk");
    assert!(!h.vis().dismissed(&id("tests")), "not shared until delivered");
    for (body, how) in [("", Carry::Submit), ("  \n ", Carry::Steer), ("", Carry::HandBack), ("[card c4f9x2a] Yes", Carry::Answer)] {
        let r = a.ride_now(J, &g, &h.vis(), body, how);
        assert_eq!((r.text.as_str(), r.carrier, r.notices.len()), (body, None, 0), "{how:?} {body:?}: never alone, never on an answer");
    }
    assert_eq!(a.riding(J, &g, &h.vis()), [(id("tests"), Seen::Dismissed, 1)], "still waiting for words");
    let submit = a.ride_now(J, &g, &h.vis(), "Fix the parser.", Carry::Submit);
    assert_eq!(submit.text, "Fix the parser.\n[block tests v1 dismissed]");
    assert!(submit.carrier.is_some());
    assert!(a.riding(J, &g, &h.vis()).is_empty(), "in flight: on no other message");
    assert_eq!(a.dismiss_now(J, &g, &id("plan"), &h.vis()), Ok(Now::Wished(2)), "one more than any version seen, its own included");
    let steer = a.ride_now(J, &g, &h.vis(), "and the lexer", Carry::Steer);
    assert_eq!(steer.text, "and the lexer\n[block plan v2 dismissed]");
    assert_ne!(steer.carrier, submit.carrier);
    h.user(&submit.text);
    h.user(&steer.text);
    let v = h.vis();
    assert!(v.dismissed(&id("tests")) && v.dismissed(&id("plan")), "the fold read both");
    assert_eq!(a.settle_now(J, &g, &v, &[]), []);
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
    a.dismiss_now(J, &g, &id("tests"), &h.vis()).unwrap();
    let steer = a.ride_now(J, &g, &h.vis(), "also this", Carry::Steer);
    assert_eq!(line(&steer), "[block tests v1 dismissed]");
    assert_eq!(a.settle_now(J, &g, &h.vis(), &[steer.carrier.unwrap()]), [], "still queued: still in flight");
    assert!(a.riding(J, &g, &h.vis()).is_empty());
    a.settle_now(J, &g, &h.vis(), &[]);
    let again = a.ride_now(J, &g, &h.vis(), "next", Carry::Submit);
    assert_eq!(again.text, "next\n[block tests v1 dismissed]", "re-armed at the same version");

    let mut b = desires(&dir("dropped-show"));
    b.dismiss_now(J, &g, &id("tests"), &h.vis()).unwrap();
    let steer = b.ride_now(J, &g, &h.vis(), "also this", Carry::Steer);
    assert_eq!(b.show_now(J, &g, &id("tests"), &h.vis()), Ok(Now::Wished(2)), "sent: a new version, not a cancel");
    assert!(!b.dismissed(J, &g, &id("tests"), &h.vis()), "drawn shown at once");
    b.settle_now(J, &g, &h.vis(), &[steer.carrier.unwrap()]);
    assert_eq!(b.riding(J, &g, &h.vis()), [(id("tests"), Seen::Shown, 2)]);
    b.settle_now(J, &g, &h.vis(), &[]);
    assert_eq!(b.riding(J, &g, &h.vis()), [(id("tests"), Seen::Shown, 2)], "the dropped one is not re-armed: the Show is the desire");
    let next = b.ride_now(J, &g, &h.vis(), "go", Carry::Submit);
    assert_eq!(next.text, "go\n[block tests v2 shown]");
    let mut h2 = h.clone();
    h2.user(&next.text);
    assert_eq!(b.settle_now(J, &g, &h2.vis(), &[]), []);
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
    a.dismiss_now(J, &g, &id("tests"), &h0.vis()).unwrap();
    let queued = a.ride_now(J, &g, &h0.vis(), "also", Carry::Steer);
    a.show_now(J, &g, &id("tests"), &h0.vis()).unwrap();
    assert_eq!(a.riding(J, &g, &h0.vis()), [(id("tests"), Seen::Shown, 2)]);
    let mut h = h0.clone();
    h.user(&queued.text);
    assert!(h.vis().dismissed(&id("tests")), "the queued dismissal landed first: shared");
    assert_eq!(a.settle_now(J, &g, &h.vis(), &[]), []);
    assert!(!a.dismissed(J, &g, &id("tests"), &h.vis()), "this desk draws its own desire meanwhile");
    let show = a.ride_now(J, &g, &h.vis(), "go on", Carry::Submit);
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
    assert_eq!(a.settle_now(J, &g, &h.vis(), &[]), []);
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
    a.dismiss_now(J, &g, &id("tests"), &h.vis()).unwrap();
    let sent = a.ride_now(J, &g, &h.vis(), "[block plan edited r1] plan → x\n{\"of\":\"…\"}", Carry::HandBack);
    assert_eq!(line(&sent), "[block tests v1 dismissed]");
    a.lost(sent.carrier.unwrap());
    assert_eq!(a.riding(J, &g, &h.vis()), [(id("tests"), Seen::Dismissed, 1)], "lost: it rides the next message, the same version");
    assert_eq!(a.show_now(J, &g, &id("tests"), &h.vis()), Ok(Now::Wished(2)), "sent once: a copy may still land, so a new version");
    let resend = a.ride_now(J, &g, &h.vis(), "[block plan edited r1] plan → x\n{\"of\":\"…\"}", Carry::HandBack);
    assert_eq!(line(&resend), "[block tests v2 shown]");
    assert!(!resend.text.contains("dismissed"));

    let mut b = desires(&dir("lost-cancel"));
    b.dismiss_now(J, &g, &id("tests"), &h.vis()).unwrap();
    assert_eq!(b.show_now(J, &g, &id("tests"), &h.vis()), Ok(Now::Cancelled));
    assert_eq!((b.len(), desires(b.path().unwrap().parent().unwrap()).len()), (0, 0), "nothing kept");
    let r = b.ride_now(J, &g, &h.vis(), "hello", Carry::Submit);
    assert_eq!((r.text.as_str(), r.carrier), ("hello", None), "nothing sent");
    // And the other way: a Show never sent over a delivered dismissal, then
    // dismiss again.
    let mut hd = h.clone();
    hd.user("x\n[block tests v1 dismissed]");
    assert_eq!(b.show_now(J, &g, &id("tests"), &hd.vis()), Ok(Now::Wished(2)));
    assert_eq!(b.dismiss_now(J, &g, &id("tests"), &hd.vis()), Ok(Now::Cancelled));
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
    a.dismiss_now(J, &g, &id("tests"), &h.vis()).unwrap();
    let old = a.ride_now(J, &g, &h.vis(), "also", Carry::Steer);
    a.show_now(J, &g, &id("tests"), &h.vis()).unwrap();
    h.user(&old.text);
    a.settle_now(J, &g, &h.vis(), &[]);
    assert!(b.dismissed(J, &g, &id("tests"), &h.vis()), "another desk: the old notice applies");
    assert!(h.vis().dismissed(&id("tests")), "and the agent read it");
    assert!(!a.dismissed(J, &g, &id("tests"), &h.vis()), "this desk: its newer desire");
    let newer = a.ride_now(J, &g, &h.vis(), "now", Carry::Submit);
    assert_eq!(line(&newer), "[block tests v2 shown]");
    h.user(&newer.text);
    a.settle_now(J, &g, &h.vis(), &[]);
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
        a.dismiss_now(J, &g, &id("tests"), &h0.vis()).unwrap();
        let mine = a.ride_now(J, &g, &h0.vis(), "mine", Carry::Submit);
        let theirs = "theirs\n[block tests v1 shown]";
        let mut h = h0.clone();
        if theirs_first {
            h.user(theirs);
            h.user(&mine.text);
        } else {
            h.user(&mine.text);
            h.user(theirs);
        }
        let fb = a.settle_now(J, &g, &h.vis(), &[]);
        assert_eq!(a.len(), 0, "settled either way: v1 is at the effective version");
        if theirs_first {
            assert_eq!(fb, [(id("tests"), Feedback::ChangedByLater)]);
            assert_eq!(a.feedback(J, &g, &id("tests")).map(Feedback::text), Some("changed by a later message"));
            assert!(!a.dismissed(J, &g, &id("tests"), &h.vis()), "the shared state drawn");
        } else {
            assert_eq!(fb, []);
            assert!(a.dismissed(J, &g, &id("tests"), &h.vis()));
        }
        // The next wish clears what was said.
        a.dismiss_now(J, &g, &id("tests"), &h.vis()).unwrap();
        assert_eq!(a.feedback(J, &g, &id("tests")), None);
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
    a.dismiss_now(J, &g, &id("tests"), &h.vis()).unwrap();
    h.ops(json!([{"op":"remove","id":"tests"}]));
    let r = a.ride_now(J, &g, &h.vis(), "ok", Carry::Submit);
    assert_eq!(line(&r), "[block tests v1 dismissed]", "an absent id rides like a live one");
    h.user(&r.text);
    assert_eq!(a.settle_now(J, &g, &h.vis(), &[]), []);
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
    a.dismiss_now(J, &g, &id("tests"), &h.vis()).unwrap();
    for body in ["see\n[block zz v9 shown]", "see\n[block tests v1 shown]"] {
        let r = a.ride_now(J, &g, &h.vis(), body, Carry::Submit);
        assert_eq!((r.text.as_str(), r.carrier), (body, None), "{body:?}: its look-alike would join the trailer");
    }
    for body in ["see\n[block zz v9 dismissed]", "[block zz v9 shown]"] {
        let mut b = desires(&dir("twice-body"));
        b.dismiss_now(J, &g, &id("tests"), &h.vis()).unwrap();
        let r = b.ride_now(J, &g, &h.vis(), body, Carry::Submit);
        let (read, t) = message::split(&r.text);
        assert_eq!((read, t.text()), (body, "\n[block tests v1 dismissed]".to_string()), "{body:?}");
    }

    let mut h = H::with(&["tests"]);
    let mut a = desires(&dir("twice-many"));
    for i in 0..10 {
        let id = id(&format!("b{i}"));
        if i % 2 == 0 {
            a.dismiss_now(J, &g, &id, &h.vis()).unwrap();
        } else {
            h.user(&format!("x\n[block b{i} v{} dismissed]", 100 + i));
            a.show_now(J, &g, &id, &h.vis()).unwrap();
        }
    }
    a.dismiss_now(J, &g, &id("tests"), &h.vis()).unwrap();
    let body = "see\n[block tests v1 shown]";
    let r = a.ride_now(J, &g, &h.vis(), body, Carry::Submit);
    let (read, t) = message::split(&r.text);
    assert_eq!(read, body, "two lines of the desk's: the look-alike is the body's");
    assert!(t.named_twice().is_empty() && t.lines.len() == 2 && t.lines[0].visibility == Seen::Dismissed);
    assert_eq!((t.lines[0].notices.len(), t.lines[1].notices.len()), (6, 5));

    let mut b = desires(&dir("twice-typed"));
    let h1 = H::with(&["tests"]);
    b.dismiss_now(J, &g, &id("tests"), &h1.vis()).unwrap();
    let mut h2 = h1.clone();
    h2.user("hm\n[block tests v1 dismissed]\n[block tests v1 shown]");
    assert_eq!(h2.vis().record(&id("tests")), None, "named twice: neither applies");
    b.settle_now(J, &g, &h2.vis(), &[]);
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
        a.dismiss_now(J, &g, &id("tests"), &h0.vis()).unwrap();
        let queued = a.ride_now(J, &g, &h0.vis(), "also", Carry::Steer);
        drop(a);
        let mut a = desires(&d);
        assert_eq!(a.desire(J, &g, &id("tests")).map(|d| (d.dismissed, d.version, d.sent, d.in_flight())), Some((true, 1, true, false)));
        let mut h = h0.clone();
        if landed {
            h.user(&queued.text);
        }
        a.settle_now(J, &g, &h.vis(), &[]);
        assert!(a.dismissed(J, &g, &id("tests"), &h.vis()));
        if landed {
            assert_eq!(a.len(), 0, "landed while the desk was down: done");
            continue;
        }
        let again = a.ride_now(J, &g, &h.vis(), "resumed", Carry::Submit);
        assert_eq!(line(&again), "[block tests v1 dismissed]", "rides again at the same version");
        h.user(&again.text);
        h.user(&queued.text);
        let s = h.state();
        assert!(s.log().any(|l| matches!(l.outcome, cards_spec::board::Outcome::Stale { version: 1, later: 1 })), "the second copy: stale");
        a.settle_now(J, &g, &h.vis(), &[]);
        assert_eq!(a.len(), 0);
    }

    // A compaction with a carrier in flight.
    let d = dir("compacted");
    let mut a = desires(&d);
    let mut p = crate::presentation::Presentation::load(Some(d.join(crate::presentation::FILE)));
    let mut h = H::with(&["tests", "plan"]);
    h.user("x\n[block plan v1 dismissed]");
    a.dismiss_now(J, &g, &id("tests"), &h.vis()).unwrap();
    let queued = a.ride_now(J, &g, &h.vis(), "also", Carry::Steer);
    a.show_now(J, &g, &id("plan"), &h.vis()).unwrap();
    a.dismiss_now(J, &g, &id("notes"), &h.vis()).unwrap();
    let soft = a.compacted_now(J, &h.state());
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

/// An id the board names nowhere but its visibility records (a notice
/// for an id never a block, its log entry pushed out by 256 later ones) is
/// carried into the successor too: drawn dismissed, so a soft dismissal.
#[test]
fn a_soft_dismissal_of_an_id_only_the_records_name() {
    let g = generation();
    let mut h = H::with(&["plan"]);
    h.user("x\n[block gone v1 dismissed]");
    for v in 2..=(cards_spec::board::LOG_MAX as u32 + 1) {
        h.user(&format!("n\n[block plan v{v} {}]", if v % 2 == 0 { "dismissed" } else { "shown" }));
    }
    let s = h.state();
    let gone = Ref::Block(id("gone"));
    assert!(s.block(&gone).is_none() && s.tombstones().all(|t| t.id != gone), "never a block");
    assert!(s.log().all(|l| l.id.as_ref() != Some(&gone)), "out of the log");
    let mut a = desires(&dir("records"));
    assert!(a.dismissed(J, &g, &id("gone"), s.visibility()), "drawn dismissed: the record");
    assert_eq!(a.compacted_now(J, &s), [id("gone")], "plan is shown");
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
    a.dismiss_now(J, &g, &id("tests"), &h0.vis()).unwrap();
    let late = a.ride_now(J, &g, &h0.vis(), "also", Carry::Steer);
    let mut h = h0.clone();
    h.ops(json!([{"op":"remove","id":"tests"}]));
    others(&mut h, "b", 65, 2, "shown");
    assert_eq!(h.vis().floor(), 2, "one record evicted: the oldest shown");
    h.user(&late.text);
    assert_eq!(h.vis().record(&id("tests")), None, "v1 under the floor: stale");
    assert_eq!(a.settle_now(J, &g, &h.vis(), &[]), [(id("tests"), Feedback::NotApplied)]);
    assert_eq!(a.feedback(J, &g, &id("tests")).map(Feedback::text), Some("not applied · dismiss again"));
    assert_eq!(a.dismiss_now(J, &g, &id("tests"), &h.vis()), Ok(Now::Wished(67)), "above everything seen");
    let r = a.ride_now(J, &g, &h.vis(), "again", Carry::Submit);
    h.user(&r.text);
    assert!(h.vis().dismissed(&id("tests")));

    // Retained at v2, the floor at 100, a delayed Show at v3.
    let mut h = H::with(&["tests"]);
    h.user("x\n[block zz v1 shown]\n[block tests v2 dismissed]");
    let mut a = desires(&dir("floor-retained"));
    assert_eq!(a.show_now(J, &g, &id("tests"), &h.vis()), Ok(Now::Wished(3)));
    others(&mut h, "c", 160, 4, "shown");
    let v = h.vis();
    assert!(v.floor() >= 100, "floor {}", v.floor());
    assert_eq!((v.effective_version(&id("tests")), v.dismissed(&id("tests"))), (2, true), "a live block's dismissal is never evicted");
    assert_eq!(a.settle_now(J, &g, &v, &[]), [], "not settled: 3 is above its record");
    assert_eq!(a.riding(J, &g, &v), [(id("tests"), Seen::Shown, 3)], "attached");
    let r = a.ride_now(J, &g, &v, "now", Carry::Submit);
    h.user(&r.text);
    assert_eq!(h.vis().record(&id("tests")).map(|r| (r.dismissed, r.version)), Some((false, 3)), "admitted");
    assert_eq!(a.settle_now(J, &g, &h.vis(), &[]), []);
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
    a.dismiss_now(J, &g, &id("tests"), board.view(&events).visibility()).unwrap();
    let mut history = base.clone();
    history.extend(wrappers);
    let (board, events) = Board::of_history(&history);
    assert_eq!(board.generation(), g);
    let v = board.view(&events).visibility().clone();
    assert_eq!(v.record(&id("tests")), None, "no wrapper is the user's text");
    a.settle_now(J, &g, &v, &[]);
    assert_eq!(a.riding(J, &g, &v), [(id("tests"), Seen::Dismissed, 1)]);
    let r = a.ride_now(J, &g, &v, "words", Carry::Submit);
    history.push(msg("user", json!(r.text)));
    let (board, events) = Board::of_history(&history);
    a.settle_now(J, &g, board.view(&events).visibility(), &[]);
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
    a.dismiss_now(J, &g, &id("tests"), &h.vis()).unwrap();
    let reattached = H { events: h.events.clone(), tools: 0 };
    a.settle_now(J, &g, &reattached.vis(), &[]);
    assert!(a.dismissed(J, &g, &id("tests"), &reattached.vis()), "reattach, undelivered: the desire");
    assert!(desires(&d).dismissed(J, &g, &id("tests"), &h.vis()), "restart, undelivered: the file");
    assert!(!b.dismissed(J, &g, &id("tests"), &h.vis()), "another desk: not yet");
    let r = a.ride_now(J, &g, &h.vis(), "go", Carry::Submit);
    h.user(&r.text);
    h.ops(json!([{"op":"patch","id":"tests","set":{"text":"y"}}]));
    a.settle_now(J, &g, &h.vis(), &[]);
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
            assert!(matches!(a.dismiss_now(&conv, &g, &id(&format!("b{i}")), &h.vis()), Ok(Now::Wished(_))), "{conv} b{i}");
        }
    }
    assert_eq!(a.len(), PER_DESK);
    let one_more = id("one-more");
    assert_eq!(a.dismiss_now(&format!("{J}-0"), &g, &one_more, &h.vis()), Err(Refused(FULL_HERE.into())));
    assert_eq!(a.dismiss_now("another", &g, &one_more, &h.vis()), Err(Refused(FULL_DESK.into())));
    assert!(!a.dismissed("another", &g, &one_more, &h.vis()), "refused before it drew");
    // A wish over one kept is no new desire: it goes in.
    assert!(matches!(a.show_now(&format!("{J}-3"), &g, &id("b5"), &h.vis()), Ok(Now::Cancelled)));
    assert!(matches!(a.dismiss_now(&format!("{J}-3"), &g, &id("b5"), &h.vis()), Ok(Now::Wished(_))));
    let before: Vec<_> = (0..8)
        .flat_map(|c| (0..64).map(move |i| (c, i)))
        .map(|(c, i)| a.desire(&format!("{J}-{c}"), &g, &id(&format!("b{i}"))).map(|d| (d.dismissed, d.version)))
        .collect();
    let text = std::fs::read_to_string(d.join(FILE)).unwrap();
    assert!(text.len() <= RESERVE, "{} bytes", text.len());
    let b = desires(&d);
    assert_eq!(b.len(), PER_DESK, "a restart loses no desire");
    let after: Vec<_> = (0..8)
        .flat_map(|c| (0..64).map(move |i| (c, i)))
        .map(|(c, i)| b.desire(&format!("{J}-{c}"), &g, &id(&format!("b{i}"))).map(|d| (d.dismissed, d.version)))
        .collect();
    assert_eq!(before, after);
    assert!(b.desire(&format!("{J}-0"), &g, &id("b0")).is_some_and(|d| !d.sent), "never sent: the file keeps the mark");
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
        match a.dismiss_now(&conv, &g, &id(&big), &h.vis()) {
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

/// The file's lock, held as another process would.
fn hold_lock(d: &Path) -> std::fs::File {
    let held = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(d.join(format!("{FILE}.lock"))).unwrap();
    held.try_lock().unwrap();
    held
}

/// `a`'s next write (one is queued).
fn a_write(a: &mut Desires) -> Write {
    a.next_write().expect("a write queued")
}

/// `w` run on a thread of its own: the background task.
fn in_background(w: Write) -> Landing {
    std::thread::spawn(move || w.run()).join().unwrap()
}

/// The UI thread never waits for the file nor writes it: with another
/// process holding its lock, the load and every call return at once,
/// nothing on this thread touches the disk, and a wish draws as before
/// (pending) and rides nothing until its write, run in the background one
/// at a time, lands.
#[test]
fn the_ui_thread_never_waits_for_the_file() {
    let h = H::with(&["tests", "plan"]);
    let g = generation();
    let d = dir("never-waits");
    let held = hold_lock(&d);
    let disk = blockfiles::on_this_thread();
    let t = std::time::Instant::now();
    let mut a = Desires::load(Some(d.join(FILE)));
    assert_eq!(a.dismiss(J, &g, &id("tests"), &h.vis()), Ok(Ask::Saving));
    assert_eq!(a.dismiss(J, &g, &id("plan"), &h.vis()), Ok(Ask::Saving));
    assert_eq!(a.dismiss(J, &g, &id("plan"), &h.vis()), Ok(Ask::Unchanged), "asked already");
    assert!(!a.dismissed(J, &g, &id("tests"), &h.vis()) && a.pending(J, &g, &id("tests")), "drawn as before, pending");
    assert!(a.riding(J, &g, &h.vis()).is_empty(), "nothing rides before it is written");
    let plain = Riding::Go(Ride { text: "go".into(), carrier: None, notices: Vec::new() });
    assert_eq!(a.ride(J, &g, &h.vis(), "go", Carry::Submit), plain, "nothing rides, nothing waits: it goes");
    a.settle(J, &g, &h.vis(), &[]);
    a.discard("another");
    a.compacted("a-third", &h.state());
    let w = a.next_write().expect("the launch's sweep");
    assert!(a.next_write().is_none(), "one out at a time");
    assert!(t.elapsed() < blockfiles::LOCK_WAIT / 2, "{:?}", t.elapsed());
    assert_eq!(blockfiles::on_this_thread(), disk, "no wait and no write on the UI thread");
    // The background task waits for the lock, let go meanwhile.
    let bg = std::thread::spawn(move || w.run());
    std::thread::sleep(blockfiles::LOCK_WAIT / 5);
    held.unlock().unwrap();
    let mut landed = a.landed(bg.join().unwrap());
    let w = a.next_write().expect("the first wish's write");
    landed.extend(a.landed(in_background(w)));
    assert!(a.dismissed(J, &g, &id("tests"), &h.vis()) && !a.dismissed(J, &g, &id("plan"), &h.vis()), "the first landed, drawn");
    while let Some(w) = a.next_write() {
        landed.extend(a.landed(in_background(w)));
    }
    assert_eq!(blockfiles::on_this_thread(), disk);
    assert_eq!(wish_done(&landed), [Ok(Done::Wished(1)), Ok(Done::Wished(2))]);
    assert!(a.dismissed(J, &g, &id("plan"), &h.vis()) && !a.pending(J, &g, &id("plan")) && !a.writing());
    assert_eq!(a.riding(J, &g, &h.vis()).len(), 2, "written: they ride");
    // A message carrying them waits for their mark, made in the background.
    let Riding::Wait(t) = a.ride(J, &g, &h.vis(), "now", Carry::Submit) else { panic!("marked first") };
    let w = a_write(&mut a);
    let sent = a.landed(in_background(w));
    assert_eq!(
        sent,
        [Landed::Send {
            ticket: t,
            ride: Ride {
                text: "now\n[block tests v1, plan v2 dismissed]".into(),
                carrier: Some(1),
                notices: vec![(id("tests"), Seen::Dismissed, 1), (id("plan"), Seen::Dismissed, 2)]
            }
        }]
    );
    assert_eq!(blockfiles::on_this_thread(), disk);
    assert_eq!(desires(&d).len(), 2);
}

/// A write that times out (the lock held past its wait), fails (the
/// disk), or is refused by the file (another process filled it) refuses
/// the wish: it never drew, the block says why until the next wish, and a
/// restart has nothing of it.
#[test]
fn a_write_that_fails_or_times_out_refuses_without_drawing() {
    let h = H::with(&["tests"]);
    let g = generation();
    let refused = |landed: &[Landed]| match landed {
        [Landed::Wish { done: Err(Refused(why)), .. }] => why.clone(),
        other => panic!("{other:?}"),
    };
    let d = dir("timed-out");
    let mut a = desires(&d);
    let held = hold_lock(&d);
    assert_eq!(a.dismiss(J, &g, &id("tests"), &h.vis()), Ok(Ask::Saving));
    let w = a.next_write().unwrap();
    let landed = a.landed(in_background(w));
    let why = refused(&landed);
    assert!(why.contains("not locked"), "{why}");
    assert!(!a.dismissed(J, &g, &id("tests"), &h.vis()) && !a.pending(J, &g, &id("tests")), "never drawn");
    assert_eq!(a.refused(J, &id("tests")), Some(why.as_str()), "said on the block");
    assert!(a.riding(J, &g, &h.vis()).is_empty() && a.len() == 0);
    held.unlock().unwrap();
    assert_eq!(desires(&d).len(), 0, "a restart has nothing of it");
    assert_eq!(a.dismiss_now(J, &g, &id("tests"), &h.vis()), Ok(Now::Wished(1)), "the lock let go: written");
    assert_eq!(a.refused(J, &id("tests")), None);

    let d = dir("unwritable");
    std::fs::write(d.join("not-a-folder"), "x").unwrap();
    let mut a = Desires::load(Some(d.join("not-a-folder").join(FILE)));
    flush(&mut a);
    assert_eq!(a.dismiss(J, &g, &id("tests"), &h.vis()), Ok(Ask::Saving));
    let why = refused(&flush(&mut a));
    assert!(why.contains("not-a-folder"), "{why}");
    assert!(!a.dismissed(J, &g, &id("tests"), &h.vis()) && a.refused(J, &id("tests")).is_some());

    let d = dir("filled");
    let (mut a, mut b) = (desires(&d), desires(&d));
    for i in 0..PER_CONVERSATION {
        b.dismiss_now(J, &g, &id(&format!("b{i}")), &h.vis()).unwrap();
    }
    assert_eq!(a.dismiss(J, &g, &id("tests"), &h.vis()), Ok(Ask::Saving), "a's memory has none of b's");
    assert_eq!(refused(&flush(&mut a)), FULL_HERE, "the file's cap, at the write");
    assert!(!a.dismissed(J, &g, &id("tests"), &h.vis()));
    assert_eq!(desires(&d).len(), PER_CONVERSATION, "b's kept, a's not written");
}

/// A write landing (the file as it wrote it) keeps what memory dropped
/// since and hasn't written yet: a desire settled meanwhile stays settled.
#[test]
fn a_landing_keeps_what_memory_dropped_since() {
    let mut h = H::with(&["tests", "plan"]);
    let g = generation();
    let d = dir("dropped-since");
    let mut a = desires(&d);
    a.dismiss_now(J, &g, &id("tests"), &h.vis()).unwrap();
    let r = a.ride_now(J, &g, &h.vis(), "go", Carry::Submit);
    assert_eq!(a.dismiss(J, &g, &id("plan"), &h.vis()), Ok(Ask::Saving));
    let w = a.next_write().unwrap();
    h.user(&r.text);
    a.settle(J, &g, &h.vis(), &[]);
    assert!(a.desire(J, &g, &id("tests")).is_none(), "settled: dropped in memory, its write queued");
    a.landed(w.run());
    assert!(a.desire(J, &g, &id("tests")).is_none(), "the plan's write had tests: still dropped");
    assert!(a.desire(J, &g, &id("plan")).is_some());
    flush(&mut a);
    assert_eq!(desires(&d).waiting(), [(J.to_string(), 1)]);
}

/// A wish on one not yet written is decided against it: dismiss then show
/// before either lands is a cancel (nothing kept, nothing rides), and they
/// land in order.
#[test]
fn a_wish_on_a_pending_one_is_decided_against_it() {
    let h = H::with(&["tests"]);
    let g = generation();
    let d = dir("pending-twice");
    let mut a = desires(&d);
    assert_eq!(a.dismiss(J, &g, &id("tests"), &h.vis()), Ok(Ask::Saving));
    assert_eq!(a.show(J, &g, &id("tests"), &h.vis()), Ok(Ask::Saving));
    assert_eq!(wish_done(&flush(&mut a)), [Ok(Done::Wished(1)), Ok(Done::Cancelled)]);
    assert!(!a.dismissed(J, &g, &id("tests"), &h.vis()) && a.len() == 0);
    assert_eq!(a.ride_now(J, &g, &h.vis(), "go", Carry::Submit).carrier, None, "nothing sent");
    assert_eq!(desires(&d).len(), 0);
}

/// No config folder: nothing can be kept, so every wish is refused.
#[test]
fn without_a_folder_every_wish_is_refused() {
    let h = H::with(&["tests"]);
    let mut a = Desires::load(None);
    assert!(a.dismiss_now(J, &generation(), &id("tests"), &h.vis()).is_err());
    assert!(a.dismiss_now("", &generation(), &id("tests"), &h.vis()).is_err());
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
    a.dismiss_now(J, &generation(), &id("tests"), &H::default().vis()).unwrap();
    let g = generation_key(&generation());
    let text = json!({"version": 2, "conversations": {J: {g: {"top": 1, "landed": 0, "owed": 0, "desires": {
        "tests": {"d": 1, "v": 1, "s": 0}, "Bad": {"d": 1, "v": 1, "s": 0}, "plan": {"d": 1, "v": 0, "s": 0}}}}}, "settled": []});
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
    a.dismiss_now(J, &generation(), &id("tests"), &H::default().vis()).unwrap();
    assert_eq!(std::fs::metadata(d.join(FILE)).unwrap().permissions().mode() & 0o777, 0o600);
}

/// Another generation's desires under the same journal (its history
/// replaced) count nowhere there, and go at the settle that sees the
/// history move past them (the desk's first settle of the conversation, or
/// one of another generation after one of theirs).
#[test]
fn another_generation_under_the_journal_drops_its_desires() {
    let h = H::with(&["tests"]);
    let (g, g2) = (generation(), Generation::of(&json!({"role":"user","content":"other"})));
    let d = dir("generation");
    let mut a = desires(&d);
    a.settle_now(J, &g, &h.vis(), &[]);
    a.dismiss_now(J, &g, &id("tests"), &h.vis()).unwrap();
    assert!(a.desire(J, &g2, &id("tests")).is_none() && !a.dismissed(J, &g2, &id("tests"), &h.vis()));
    a.settle_now(J, &g2, &h.vis(), &[]);
    assert_eq!((a.len(), desires(&d).len()), (0, 0), "seen moving past: dropped");
    let mut b = desires(&d);
    a.dismiss_now(J, &g, &id("tests"), &h.vis()).unwrap();
    b.settle_now(J, &g2, &h.vis(), &[]);
    assert_eq!(desires(&d).len(), 1, "b knew none of it: its first settle drops what it knew, none");
    let mut c = desires(&d);
    c.settle_now(J, &g2, &h.vis(), &[]);
    assert_eq!((c.len(), desires(&d).len()), (0, 0), "c's first settle, on g2: g's go");
}

/// Two desks on one journal at different generations (its history
/// replaced, one desk behind): the behind desk's wish, send, settle and
/// compaction touch only its own generation, never the other's desires
/// nor their marks.
#[test]
fn a_stale_generation_touches_no_other_generation() {
    let h = H::with(&["tests", "plan", "notes"]);
    let (g1, g2) = (generation(), Generation::of(&json!({"role":"user","content":"replaced"})));
    let d = dir("generations");
    let (mut old, mut new) = (desires(&d), desires(&d));
    old.settle_now(J, &g1, &h.vis(), &[]);
    old.dismiss_now(J, &g1, &id("tests"), &h.vis()).unwrap();
    new.settle_now(J, &g2, &h.vis(), &[]);
    assert_eq!(new.dismiss_now(J, &g2, &id("tests"), &h.vis()), Ok(Now::Wished(1)));
    let kept = |what: &str| {
        let f = desires(&d);
        assert_eq!(f.desire(J, &g2, &id("tests")).map(|x| (x.dismissed, x.version, x.sent)), Some((true, 1, false)), "{what}");
    };
    old.dismiss_now(J, &g1, &id("plan"), &h.vis()).unwrap();
    kept("after the old generation's wish");
    let r = old.ride_now(J, &g1, &h.vis(), "old", Carry::Submit);
    assert_eq!(line(&r), "[block tests v1, plan v2 dismissed]");
    kept("after the old generation's send");
    let mut h1 = h.clone();
    h1.user(&r.text);
    old.settle_now(J, &g1, &h1.vis(), &[]);
    kept("after the old generation's settle");
    old.dismiss_now(J, &g1, &id("notes"), &h1.vis()).unwrap();
    assert_eq!(old.compacted_now(J, &h1.state()), [id("notes"), id("plan"), id("tests")]);
    kept("after the old generation's compaction");
    old.dismiss_now(J, &g1, &id("notes"), &h.vis()).unwrap();
    old.settle_now(J, &g2, &h.vis(), &[]);
    kept("after the old desk saw the history move: its g1 only");
    assert!(desires(&d).desire(J, &g1, &id("notes")).is_none());
}

/// The chip's `×` keeps a notice back from the next message only; past 8
/// of a kind, the rest ride the next message, the oldest versions first.
#[test]
fn the_chip_keeps_one_back_and_eight_ride_at_most() {
    let h = H::default();
    let g = generation();
    let mut a = desires(&dir("chip"));
    for i in 0..11 {
        a.dismiss_now(J, &g, &id(&format!("b{i:02}")), &h.vis()).unwrap();
    }
    assert!(a.hold(J, &g, &id("b00"), &h.vis()));
    assert!(!a.hold(J, &g, &id("zz"), &h.vis()));
    let first = a.ride_now(J, &g, &h.vis(), "one", Carry::Submit);
    assert_eq!(line(&first), "[block b01 v2, b02 v3, b03 v4, b04 v5, b05 v6, b06 v7, b07 v8, b08 v9 dismissed]");
    let second = a.ride_now(J, &g, &h.vis(), "two", Carry::Submit);
    assert_eq!(line(&second), "[block b00 v1, b09 v10, b10 v11 dismissed]", "the held one rides the message after");
}

// Two desks on one file: processes.

/// A wish of `conv`'s, as the board drew none of `id`.
fn wish(conv: &str, i: &str) -> Wish {
    Wish {
        conv: conv.into(),
        generation: generation_key(&generation()),
        id: id(i),
        dismiss: true,
        shared_dismissed: false,
        highest: 0,
        had: false,
    }
}

/// Waits for `path` (another process's signal), up to a minute.
fn signalled(path: &Path) -> bool {
    let t = std::time::Instant::now();
    while !path.exists() {
        if t.elapsed() > std::time::Duration::from_secs(60) {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    true
}

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
            // The parent holds the file's lock mid-write: this desk's wish
            // is refused (the lock not had in its wait), said; let go, it
            // goes in, merged into what the parent wrote.
            assert!(signalled(&d.join("holding")), "the parent never held the lock");
            let mut a = desires(&d);
            match a.dismiss_now("child", &generation(), &id("c0"), &h.vis()) {
                Err(Refused(why)) => std::fs::write(d.join("refused"), why).unwrap(),
                other => {
                    std::fs::write(d.join("written"), format!("{other:?}")).unwrap();
                    panic!("written under another process's lock: {other:?}");
                }
            }
            assert!(signalled(&d.join("go")), "the parent never let go");
            assert_eq!(a.dismiss_now("child", &generation(), &id("c0"), &h.vis()), Ok(Now::Wished(1)));
        }
        "crash" => {
            let w = wish("child", "crashed");
            let _ = blockfiles::update_then::<Book>(
                &d.join(FILE),
                |b| decide(b, &w).map(|_| ()),
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

/// Two desk processes on one desire file, by a handshake, no sleep: while
/// one is mid-write (its temporary file written, not yet renamed, the lock
/// held), the other's wish is refused, and the first lets go only once it
/// hears so; then the other's wish merges into what the first wrote. A
/// writer that took no lock would write instead, and say so.
#[test]
fn a_second_process_is_refused_mid_write_then_merges_into_it() {
    let d = dir("processes");
    let child = spawn("merge", &d);
    let mine = wish("parent", "p0");
    let held = blockfiles::update_then::<Book>(
        &d.join(FILE),
        |b| decide(b, &mine).map(|_| ()),
        || {
            std::fs::write(d.join("holding"), "")?;
            let t = std::time::Instant::now();
            while !d.join("refused").exists() && !d.join("written").exists() {
                if t.elapsed() > std::time::Duration::from_secs(60) {
                    return Err(std::io::Error::other("no answer from the second process"));
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            Ok(())
        },
    );
    assert!(held.is_ok(), "{held:?}");
    std::fs::write(d.join("go"), "").unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let why = std::fs::read_to_string(d.join("refused")).unwrap();
    assert!(why.contains("not locked"), "{why}");
    assert_eq!(desires(&d).waiting(), [("child".to_string(), 1), ("parent".to_string(), 1)]);
}

/// A desk killed between writing its temporary file and the rename: the
/// file is as it was, and the next load sweeps the leftover.
#[test]
fn a_crash_mid_write_keeps_the_file_and_the_next_load_sweeps() {
    let d = dir("crash");
    let mut a = desires(&d);
    a.dismiss_now(J, &generation(), &id("tests"), &H::default().vis()).unwrap();
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
    a.dismiss_now(J, &g, &id("tests"), &h.vis()).unwrap();
    a.ride_now(J, &g, &h.vis(), "go", Carry::Submit);
    assert_eq!(a.show_now(J, &g, &id("tests"), &h.vis()), Ok(Now::Wished(2)));
    assert_eq!(b.dismiss_now(J, &g, &id("tests"), &h.vis()), Ok(Now::Wished(3)), "b had seen none of it");
    assert!(b.dismissed(J, &g, &id("tests"), &h.vis()), "drawn as clicked");
    a.dismiss_now(J, &g, &id("plan"), &h.vis()).unwrap();
    assert_eq!(
        a.desire(J, &g, &id("tests")).map(|d| (d.dismissed, d.version, d.sent)),
        Some((true, 3, true)),
        "a takes it at its next write: another process's, it may have been sent"
    );
    assert_eq!(b.dismiss_now(J, &g, &id("tests"), &h.vis()), Ok(Now::Unchanged));
}

/// Two desks in one process (two windows' stores) on one file: a desk's
/// settle drops only what it settled, and the other's wishes survive it.
#[test]
fn a_second_desk_on_the_file_keeps_the_others_wishes() {
    let d = dir("second");
    let mut h = H::with(&["tests", "plan"]);
    let g = generation();
    let (mut a, mut b) = (desires(&d), desires(&d));
    a.dismiss_now(J, &g, &id("tests"), &h.vis()).unwrap();
    b.dismiss_now(J, &g, &id("plan"), &h.vis()).unwrap();
    assert_eq!(b.len(), 2, "the write took the file as it was");
    let r = a.ride_now(J, &g, &h.vis(), "go", Carry::Submit);
    assert_eq!(line(&r), "[block tests v1 dismissed]", "a's desire, a's carrier");
    h.user(&r.text);
    a.settle_now(J, &g, &h.vis(), &[]);
    let c = desires(&d);
    assert_eq!(c.desire(J, &g, &id("plan")).map(|d| d.version), Some(2), "minted above the file's, a's included");
    assert!(c.desire(J, &g, &id("tests")).is_none());
}

// Shady's review of #103 (bba9546): each of these fails there.

/// Dismiss (sent), Show, Dismiss, Show, the last three unsent: the first
/// notice may still land, so the final Show is a newer `shown` notice,
/// never a cancel to nothing. Whether the first lands before the next send
/// or after it, and across a restart before the final Show (P2-1).
#[test]
fn a_superseded_notice_still_in_flight_keeps_the_final_show() {
    let h0 = H::with(&["tests"]);
    let g = generation();
    for (restart, early) in [(false, false), (false, true), (true, false)] {
        let d = dir(&format!("four-clicks-{restart}-{early}"));
        let mut a = desires(&d);
        a.dismiss_now(J, &g, &id("tests"), &h0.vis()).unwrap();
        let first = a.ride_now(J, &g, &h0.vis(), "also", Carry::Steer);
        assert_eq!(line(&first), "[block tests v1 dismissed]");
        assert_eq!(a.show_now(J, &g, &id("tests"), &h0.vis()), Ok(Now::Wished(2)));
        assert_eq!(a.dismiss_now(J, &g, &id("tests"), &h0.vis()), Ok(Now::Wished(3)), "v1 may land: a new version");
        if restart {
            a = desires(&d);
        }
        assert_eq!(a.show_now(J, &g, &id("tests"), &h0.vis()), Ok(Now::Wished(4)), "v1 may still land: never a cancel");
        assert!(!a.dismissed(J, &g, &id("tests"), &h0.vis()));
        let mut h = h0.clone();
        if early {
            h.user(&first.text);
            a.settle_now(J, &g, &h.vis(), &[]);
            assert!(h.vis().dismissed(&id("tests")) && !a.dismissed(J, &g, &id("tests"), &h.vis()), "this desk draws its own");
        }
        let next = a.ride_now(J, &g, &h.vis(), "go", Carry::Submit);
        assert_eq!(line(&next), "[block tests v4 shown]");
        h.user(&next.text);
        if !early {
            h.user(&first.text);
        }
        assert!(!h.vis().dismissed(&id("tests")), "restart {restart}, early {early}: the final Show holds");
        a.settle_now(J, &g, &h.vis(), &[]);
        assert_eq!(a.len(), 0);
    }
}

/// Cancellation is decided against the file, never this desk's memory: B
/// sends A's unsent dismissal between A's dismiss and Show, so A's Show is
/// a new version (B's copy may land); and with B's later dismissal (sent)
/// on the file, A's Show supersedes it, and says what draws (P2-2).
#[test]
fn cancellation_is_decided_against_the_file() {
    let h0 = H::with(&["tests"]);
    let g = generation();
    let d = dir("cancel-sent");
    let mut a = desires(&d);
    a.dismiss_now(J, &g, &id("tests"), &h0.vis()).unwrap();
    let mut b = desires(&d);
    let carried = b.ride_now(J, &g, &h0.vis(), "from b", Carry::Submit);
    assert_eq!(line(&carried), "[block tests v1 dismissed]", "b carries what the file has");
    assert_eq!(a.show_now(J, &g, &id("tests"), &h0.vis()), Ok(Now::Wished(2)), "b sent it: a new version, never a cancel");
    let mut h = h0.clone();
    h.user(&carried.text);
    let next = a.ride_now(J, &g, &h.vis(), "go", Carry::Submit);
    assert_eq!(line(&next), "[block tests v2 shown]");
    h.user(&next.text);
    assert!(!h.vis().dismissed(&id("tests")), "the Show holds");

    let d = dir("cancel-later");
    let mut a = desires(&d);
    a.dismiss_now(J, &g, &id("tests"), &h0.vis()).unwrap();
    let mut b = desires(&d);
    let sent = b.ride_now(J, &g, &h0.vis(), "from b", Carry::Submit);
    assert_eq!(b.show_now(J, &g, &id("tests"), &h0.vis()), Ok(Now::Wished(2)));
    assert_eq!(b.dismiss_now(J, &g, &id("tests"), &h0.vis()), Ok(Now::Wished(3)));
    assert_eq!(a.show_now(J, &g, &id("tests"), &h0.vis()), Ok(Now::Wished(4)), "decided against b's v3, sent");
    assert!(!a.dismissed(J, &g, &id("tests"), &h0.vis()), "and it draws what it said");
    let restarted = desires(&d);
    assert_eq!(restarted.desire(J, &g, &id("tests")).map(|x| (x.dismissed, x.version, x.sent)), Some((false, 4, true)));
    let mut h = h0.clone();
    h.user(&sent.text);
    let next = a.ride_now(J, &g, &h.vis(), "go", Carry::Submit);
    assert_eq!(line(&next), "[block tests v4 shown]");
}

/// The user's discard drops the desires, not what they minted nor what
/// they owe: a dismissal sent and still in flight, discarded, then the
/// block dismissed and shown again. The new dismissal mints above the
/// discarded v1 (a second v1 would be stale), and the Show can't cancel
/// to nothing (v1 may still land): it is a notice, and it holds.
#[test]
fn a_discard_keeps_the_versions_and_what_may_still_land() {
    let h0 = H::with(&["tests"]);
    let g = generation();
    for restart in [false, true] {
        let d = dir(&format!("discard-owed-{restart}"));
        let mut a = desires(&d);
        a.dismiss_now(J, &g, &id("tests"), &h0.vis()).unwrap();
        let first = a.ride_now(J, &g, &h0.vis(), "also", Carry::Steer);
        assert_eq!(line(&first), "[block tests v1 dismissed]");
        assert_eq!(a.discard(J), 1);
        flush(&mut a);
        if restart {
            a = desires(&d);
        }
        assert_eq!(a.len(), 0);
        assert_eq!(a.dismiss_now(J, &g, &id("tests"), &h0.vis()), Ok(Now::Wished(2)), "above the discarded v1");
        assert_eq!(a.show_now(J, &g, &id("tests"), &h0.vis()), Ok(Now::Wished(3)), "v1 may land: never a cancel");
        let next = a.ride_now(J, &g, &h0.vis(), "go", Carry::Submit);
        assert_eq!(line(&next), "[block tests v3 shown]");
        let mut h = h0.clone();
        h.user(&next.text);
        h.user(&first.text);
        assert!(!h.vis().dismissed(&id("tests")), "restart {restart}: the Show holds");
    }
}

/// What a board older than a settled history drew decides nothing: A's
/// dismissal is sent and settled (dismissed in the history, gone from the
/// file); B, whose board hasn't seen that history yet (its update on the
/// way), dismisses the block (drawn shown there) and shows it again. The
/// dismissal mints above the settled v1, and the Show is no cancel but a
/// notice, which holds (found by the two-desk model: seed 89 of 1,500,
/// seed 4,011 of 30,000).
#[test]
fn a_board_older_than_a_settle_decides_no_cancel() {
    let h0 = H::with(&["tests"]);
    let g = generation();
    let d = dir("older-board");
    let mut a = desires(&d);
    a.dismiss_now(J, &g, &id("tests"), &h0.vis()).unwrap();
    let r = a.ride_now(J, &g, &h0.vis(), "from a", Carry::Submit);
    let mut h = h0.clone();
    h.user(&r.text);
    a.settle_now(J, &g, &h.vis(), &[]);
    assert_eq!(desires(&d).len(), 0, "settled: gone from the file");
    let mut b = desires(&d);
    assert!(!b.dismissed(J, &g, &id("tests"), &h0.vis()), "b's board is older: drawn shown");
    assert_eq!(b.dismiss_now(J, &g, &id("tests"), &h0.vis()), Ok(Now::Wished(2)), "above the settled v1");
    assert_eq!(b.show_now(J, &g, &id("tests"), &h0.vis()), Ok(Now::Wished(3)), "a notice, not a cancel");
    let next = b.ride_now(J, &g, &h0.vis(), "more", Carry::Submit);
    assert_eq!(line(&next), "[block tests v3 shown]");
    h.user(&next.text);
    assert!(!h.vis().dismissed(&id("tests")), "the Show holds");
}

/// Admission is decided under the lock, on the file as the wish leaves it:
/// a desk whose memory still has an id the file no longer has can't make it
/// the 65th of a conversation, nor the 513th of the desk. The refusal draws
/// nothing new, and the desk takes the file (P2-4).
#[test]
fn a_stale_existing_id_cant_pass_the_caps() {
    let g = generation();
    let h = H::with(&["tests"]);
    for desk_wide in [false, true] {
        let d = dir(&format!("stale-caps-{desk_wide}"));
        let mut a = desires(&d);
        a.dismiss_now(J, &g, &id("tests"), &h.vis()).unwrap();
        let r = a.ride_now(J, &g, &h.vis(), "go", Carry::Submit);
        let mut hd = h.clone();
        hd.user(&r.text);
        let mut b = desires(&d);
        b.settle_now(J, &g, &hd.vis(), &[]);
        let here = if desk_wide { PER_CONVERSATION - 1 } else { PER_CONVERSATION };
        for i in 0..here {
            b.dismiss_now(J, &g, &id(&format!("b{i}")), &hd.vis()).unwrap();
        }
        if desk_wide {
            for i in 0..PER_DESK - here {
                b.dismiss_now(&format!("{J}-{}", i / PER_CONVERSATION), &g, &id(&format!("o{i}")), &hd.vis()).unwrap();
            }
        }
        assert_eq!(b.len(), if desk_wide { PER_DESK } else { PER_CONVERSATION });
        let before = std::fs::read(d.join(FILE)).unwrap();
        assert!(a.desire(J, &g, &id("tests")).is_some(), "a's memory still has it");
        assert_eq!(a.show(J, &g, &id("tests"), &hd.vis()), Ok(Ask::Saving), "in memory, an update: no preflight refusal");
        let full = if desk_wide { FULL_DESK } else { FULL_HERE };
        assert_eq!(wish_done(&flush(&mut a)), [Err(Refused(full.into()))]);
        assert!(a.desire(J, &g, &id("tests")).is_none() && a.dismissed(J, &g, &id("tests"), &hd.vis()), "the file's, drawn");
        assert_eq!(std::fs::read(d.join(FILE)).unwrap(), before, "nothing written");
        assert_eq!(desires(&d).len(), b.len(), "a restart has the same");
    }
}

/// A file made exactly `target` bytes long, of one-desire conversations
/// keyed to fit: each desire dismissed at `v`, sent; no settled mark.
fn edge_book(target: usize, v: u32) -> Book {
    let gk = generation_key(&generation());
    let add = |b: &mut Book, key: &str| {
        let here = b.generation_mut(key, &gk);
        here.top = v;
        here.desires.insert(id(&format!("{:x<24}", "t")), Desire::new(true, v, true));
    };
    let size_with = |b: &Book, key: &str| {
        let mut c = b.clone();
        add(&mut c, key);
        c.text().len()
    };
    let key = |n: usize, len: usize| format!("{n:04}{}", "k".repeat(len - 4));
    let mut book = Book::default();
    let mut n = 0;
    while size_with(&book, &key(n, KEY_MAX)) <= target {
        add(&mut book, &key(n, KEY_MAX));
        n += 1;
    }
    let mut least = size_with(&book, &key(n, 4));
    if least > target {
        // Too little room for the last: the one before gives some back.
        let k = least - target;
        book.conversations.remove(&key(n - 1, KEY_MAX));
        add(&mut book, &key(n - 1, KEY_MAX - k));
        least = size_with(&book, &key(n, 4));
    }
    add(&mut book, &key(n, 4 + target - least));
    assert_eq!(book.text().len(), target);
    assert!(book.settled.is_empty());
    book
}

/// The reserve on the file as written, version widths counted: a file
/// exactly at 64 KiB refuses a Show of one of its desires that would
/// write v10 over v9 (bytes more): nothing drawn, nothing written, and
/// a restart has it the same. A new id is admitted only if every desire
/// could still be superseded at its widest version (P2-4).
#[test]
fn the_reserve_counts_a_versions_width() {
    let g = generation();
    let d = dir("reserve-width");
    let book = edge_book(RESERVE, 9);
    std::fs::write(d.join(FILE), book.text()).unwrap();
    let mut a = desires(&d);
    assert_eq!(a.len(), book.len());
    let (conv, slot) = book.conversations.iter().next().map(|(c, g)| (c.clone(), g.values().next().unwrap().desires.clone())).unwrap();
    let t = slot.keys().next().unwrap().clone();
    let shared = H::with(&[t.as_str()]).vis();
    assert!(a.dismissed(&conv, &g, &t, &shared));
    assert_eq!(a.show(&conv, &g, &t, &shared), Ok(Ask::Saving));
    let refused = wish_done(&flush(&mut a));
    // v10 over v9, and its generation's top: two bytes more.
    assert!(matches!(&refused[..], [Err(Refused(why))] if why.starts_with(FULL_DESK) && why.contains("65538 bytes")), "{refused:?}");
    assert!(a.dismissed(&conv, &g, &t, &shared), "nothing drawn");
    assert_eq!(std::fs::read_to_string(d.join(FILE)).unwrap(), book.text(), "nothing written");
    assert!(desires(&d).dismissed(&conv, &g, &t, &shared));

    // Admission at the widest: a book whose desires fit now but not at
    // their widest takes no new id; one that fits at the widest takes it,
    // and every desire can then go to VERSION_MAX.
    let d = dir("reserve-widest");
    let narrow = edge_book(RESERVE - 1_000, 1);
    assert!(narrow.widest_bytes() > RESERVE && narrow.text().len() < RESERVE);
    std::fs::write(d.join(FILE), narrow.text()).unwrap();
    let mut a = desires(&d);
    let refused = wish_done(&{
        a.dismiss(J, &g, &id("new"), &H::default().vis()).unwrap();
        flush(&mut a)
    });
    assert!(matches!(&refused[..], [Err(Refused(why))] if why.contains("at the widest")), "{refused:?}");
    let mut fits = edge_book(RESERVE - 1_000, 1);
    while fits.widest_bytes() + 200 > RESERVE {
        let k = fits.conversations.keys().next().unwrap().clone();
        fits.conversations.remove(&k);
    }
    let d = dir("reserve-fits");
    std::fs::write(d.join(FILE), fits.text()).unwrap();
    let mut a = desires(&d);
    assert_eq!(a.dismiss_now(J, &g, &id("new"), &H::default().vis()), Ok(Now::Wished(1)));
    let mut wide = desires(&d).book.clone();
    wide.desires_mut().for_each(|x| x.version = VERSION_MAX);
    wide.conversations.values_mut().flat_map(|g| g.values_mut()).for_each(|x| x.top = VERSION_MAX);
    assert!(wide.text().len() <= RESERVE, "every admitted desire can be superseded: {}", wide.text().len());
}

/// Settled feedback is bounded per conversation and in all, goes once
/// seen, and goes with its generation and its conversation, desires or
/// none (P2-5).
#[test]
fn settled_feedback_is_bounded_and_goes() {
    let g = generation();
    let mut a = desires(&dir("feedback"));
    let lose = |a: &mut Desires, conv: &str, prefix: &str| {
        let mut h = H::default();
        for i in 0..PER_CONVERSATION {
            a.dismiss_now(conv, &g, &id(&format!("{prefix}{i}")), &h.vis()).unwrap();
        }
        // Another desk's later Shows of every one of them.
        others(&mut h, prefix, PER_CONVERSATION, 1_000, "shown");
        assert_eq!(a.settle_now(conv, &g, &h.vis(), &[]).len(), PER_CONVERSATION);
        assert_eq!(a.len(), 0);
        h
    };
    for batch in 0..4 {
        lose(&mut a, J, &format!("f{batch}x"));
        assert!(a.notes().0 <= FEEDBACK_PER_CONVERSATION, "batch {batch}: {:?}", a.notes());
    }
    assert_eq!(a.feedback(J, &g, &id("f3x0")), Some(Feedback::ChangedByLater));
    assert_eq!(a.feedback(J, &g, &id("f0x0")), None, "the oldest went");
    a.feedback_seen(J, &g, &id("f3x0"));
    assert_eq!(a.feedback(J, &g, &id("f3x0")), None, "seen: gone");
    for c in 0..PER_DESK / PER_CONVERSATION + 1 {
        lose(&mut a, &format!("{J}-{c}"), "e");
    }
    assert!(a.notes().0 <= FEEDBACK_MAX, "{:?}", a.notes());
    // A generation seen gone, and a compaction, with no desire left.
    let h = lose(&mut a, J, "g");
    let g2 = Generation::of(&json!({"role":"user","content":"replaced"}));
    a.settle_now(J, &g2, &H::default().vis(), &[]);
    assert_eq!(a.feedback(J, &g, &id("g0")), None, "its generation went");
    lose(&mut a, "c1", "h");
    assert!(a.feedback("c1", &g, &id("h0")).is_some());
    a.compacted_now("c1", &h.state());
    assert_eq!(a.feedback("c1", &g, &id("h0")), None, "its conversation compacted");
}

/// A cancel not landed yet keeps the desire it undoes off the next
/// message: it rides nothing while pending.
#[test]
fn a_pending_cancel_rides_nothing() {
    let h = H::with(&["tests"]);
    let g = generation();
    let mut a = desires(&dir("pending-cancel"));
    a.dismiss_now(J, &g, &id("tests"), &h.vis()).unwrap();
    assert_eq!(a.show(J, &g, &id("tests"), &h.vis()), Ok(Ask::Saving));
    assert!(a.dismissed(J, &g, &id("tests"), &h.vis()) && a.pending(J, &g, &id("tests")));
    assert!(a.riding(J, &g, &h.vis()).is_empty(), "its cancel pending: off the chip");
    let r = a.ride_now(J, &g, &h.vis(), "go", Carry::Submit);
    assert_eq!((r.text.as_str(), r.carrier), ("go", None));
    assert_eq!(wish_done(&flush(&mut a)), [Ok(Done::Cancelled)]);
    assert!(!a.dismissed(J, &g, &id("tests"), &h.vis()) && a.len() == 0);
}

/// A message marks and carries only the notices the file still has as it
/// picked them: B supersedes A's sent dismissal while A's message waits
/// for its mark, so A's message goes plain, and B's Show (already an
/// obligation, its sent mark inherited) rides A's next message.
#[test]
fn a_send_carries_only_what_the_file_still_has() {
    let h = H::with(&["tests"]);
    let g = generation();
    let d = dir("send-checks-the-file");
    let mut a = desires(&d);
    a.dismiss_now(J, &g, &id("tests"), &h.vis()).unwrap();
    let first = a.ride_now(J, &g, &h.vis(), "first", Carry::Steer);
    a.lost(first.carrier.unwrap());
    let mut b = desires(&d);
    let Riding::Wait(t) = a.ride(J, &g, &h.vis(), "second", Carry::Submit) else { panic!("waits for its mark") };
    let w = a.next_write().unwrap();
    assert_eq!(b.show_now(J, &g, &id("tests"), &h.vis()), Ok(Now::Wished(2)));
    let sent = a.landed(in_background(w)).into_iter().find_map(|l| match l {
        Landed::Send { ticket, ride } if ticket == t => Some(ride),
        _ => None,
    });
    let sent = sent.expect("its mark landed");
    assert_eq!((sent.text.as_str(), sent.carrier), ("second", None), "v1 is superseded in the file: nothing rides");
    let next = a.ride_now(J, &g, &h.vis(), "third", Carry::Submit);
    assert_eq!(line(&next), "[block tests v2 shown]");
}

/// A landed wish says what draws: a compaction made while the wish's write
/// was out drops the generation, so the wish, decided in the file, doesn't
/// draw, and says so (never `Wished` for a desire that isn't there).
#[test]
fn a_landed_wish_says_what_draws() {
    let h = H::with(&["tests"]);
    let g = generation();
    let d = dir("landed-says");
    let mut a = desires(&d);
    flush(&mut a);
    assert_eq!(a.dismiss(J, &g, &id("tests"), &h.vis()), Ok(Ask::Saving));
    let w = a.next_write().unwrap();
    a.compacted(J, &h.state());
    let done = wish_done(&a.landed(in_background(w)));
    assert_eq!(done, [Err(Refused(MOVED.into()))]);
    assert!(!a.dismissed(J, &g, &id("tests"), &h.vis()) && a.refused(J, &id("tests")) == Some(MOVED));
    flush(&mut a);
    assert_eq!(desires(&d).len(), 0, "nor in the file");
}

/// A tiny xorshift: the model's dice.
struct Dice(u64);

impl Dice {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}

/// A model of two desks on one file, one generation: random dismisses and
/// shows on either, their writes landing at once or later (or lost to a
/// restart, or failing), sends delivered in order, late, out of order or
/// dropped, settles, restarts. Once everything is sent, delivered and
/// settled, every id draws, in the history and on both desks, the state of
/// the last wish the file took.
#[test]
fn two_desks_on_one_file_end_as_the_last_wish_the_file_took() {
    let names = ["a", "b", "c"];
    let g = generation();
    // `SYNAPS_D6B_SEEDS` runs more (a deeper look); 40 by default.
    let seeds = std::env::var("SYNAPS_D6B_SEEDS").ok().and_then(|n| n.parse().ok()).unwrap_or(40u64);
    let first = std::env::var("SYNAPS_D6B_SEED").ok().and_then(|n| n.parse().ok()).unwrap_or(1u64);
    let steps = std::env::var("SYNAPS_D6B_STEPS").ok().and_then(|n| n.parse().ok()).unwrap_or(60usize);
    for seed in first..=seeds.max(first) {
        let d = dir(&format!("model-{seed}"));
        let bad = d.join("not-a-folder").join(FILE);
        std::fs::write(d.join("not-a-folder"), "x").unwrap();
        let mut dice = Dice(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let mut h = H::with(&names);
        let mut desks = [desires(&d), desires(&d)];
        let mut want: BTreeMap<BlockId, bool> = BTreeMap::new();
        let mut net: Vec<(usize, Ride)> = Vec::new();
        let mut trace = Vec::new();
        // Each desk's wishes queued, in order: they land in that order.
        let land = |desk: &mut Desires,
                    fail: bool,
                    want: &mut BTreeMap<BlockId, bool>,
                    asked: &mut VecDeque<(BlockId, bool)>,
                    trace: &mut Vec<String>| {
            let mut rides = Vec::new();
            while let Some(mut w) = desk.next_write() {
                if fail {
                    w.path = bad.clone();
                }
                for l in desk.landed(w.run()) {
                    match l {
                        Landed::Wish { id, done, .. } => {
                            let (was, dismiss) = asked.pop_front().expect("a wish queued");
                            trace.push(format!(
                                "  landed {id} {}: {done:?}{}",
                                if dismiss { "x" } else { "u" },
                                if fail { " (failing)" } else { "" }
                            ));
                            assert_eq!(was, id, "in order");
                            if done.is_ok() {
                                want.insert(id, dismiss);
                            }
                        }
                        Landed::Send { ride, .. } => {
                            trace.push(format!("  sends {:?}", ride.text));
                            rides.push(ride);
                        }
                    }
                }
            }
            rides
        };
        let mut asked: [VecDeque<(BlockId, bool)>; 2] = Default::default();
        for _ in 0..steps {
            let k = dice.below(2);
            let i = id(names[dice.below(names.len())]);
            match dice.below(12) {
                0..=4 => {
                    let dismiss = dice.below(2) == 0;
                    let r = if dismiss { desks[k].dismiss(J, &g, &i, &h.vis()) } else { desks[k].show(J, &g, &i, &h.vis()) };
                    trace.push(format!("{k} {} {i}: {r:?}", if dismiss { "x" } else { "u" }));
                    if r == Ok(Ask::Saving) {
                        asked[k].push_back((i, dismiss));
                        if dice.below(3) > 0 {
                            let fail = dice.below(6) == 0;
                            let rides = land(&mut desks[k], fail, &mut want, &mut asked[k], &mut trace);
                            net.extend(rides.into_iter().map(|r| (k, r)));
                        }
                    }
                }
                5..=6 => {
                    let body = format!("m{}", trace.len());
                    match desks[k].ride(J, &g, &h.vis(), &body, Carry::Submit) {
                        Riding::Go(r) => {
                            trace.push(format!("{k} sends {:?}", r.text));
                            net.push((k, r));
                        }
                        Riding::Wait(_) => {
                            let rides = land(&mut desks[k], false, &mut want, &mut asked[k], &mut trace);
                            net.extend(rides.into_iter().map(|r| (k, r)));
                        }
                    }
                }
                7..=8 if !net.is_empty() => {
                    let (from, r) = net.remove(dice.below(net.len()));
                    if dice.below(5) == 0 {
                        trace.push(format!("dropped {:?}", r.text));
                        if let Some(c) = r.carrier {
                            desks[from].lost(c);
                        }
                    } else {
                        trace.push(format!("delivered {:?}", r.text));
                        h.user(&r.text);
                    }
                }
                9..=10 => {
                    trace.push(format!("{k} settles"));
                    let flying: Vec<u64> = net.iter().filter(|(f, _)| *f == k).filter_map(|(_, r)| r.carrier).collect();
                    desks[k].settle(J, &g, &h.vis(), &flying);
                    let rides = land(&mut desks[k], false, &mut want, &mut asked[k], &mut trace);
                    net.extend(rides.into_iter().map(|r| (k, r)));
                }
                _ => {
                    trace.push(format!("{k} restarts"));
                    desks[k] = desires(&d);
                    asked[k].clear();
                    // Its messages already sent are the network's now.
                    for (f, r) in &mut net {
                        if *f == k {
                            r.carrier = None;
                        }
                    }
                }
            }
        }
        // Everything written, sent, delivered and settled.
        for round in 0..20 {
            for k in 0..2 {
                let rides = land(&mut desks[k], false, &mut want, &mut asked[k], &mut trace);
                net.extend(rides.into_iter().map(|r| (k, r)));
                let rides = match desks[k].ride(J, &g, &h.vis(), "drain", Carry::Submit) {
                    Riding::Go(r) => vec![r],
                    Riding::Wait(_) => land(&mut desks[k], false, &mut want, &mut asked[k], &mut trace),
                };
                net.extend(rides.into_iter().map(|r| (k, r)));
            }
            for (_, r) in net.drain(..) {
                h.user(&r.text);
            }
            for desk in &mut desks {
                desk.settle(J, &g, &h.vis(), &[]);
            }
            for k in 0..2 {
                land(&mut desks[k], false, &mut want, &mut asked[k], &mut trace);
            }
            if desks.iter().all(|x| x.len() == 0) {
                break;
            }
            assert!(round < 19, "seed {seed}: never settled\n{}", trace.join("\n"));
        }
        let shared = h.vis();
        for n in names {
            let i = id(n);
            let end = want.get(&i).copied().unwrap_or(false);
            assert_eq!(shared.dismissed(&i), end, "seed {seed}, {n}: the history\n{}", trace.join("\n"));
            for desk in &desks {
                assert_eq!(desk.dismissed(J, &g, &i, &shared), end, "seed {seed}, {n}: a desk\n{}", trace.join("\n"));
            }
        }
    }
}
