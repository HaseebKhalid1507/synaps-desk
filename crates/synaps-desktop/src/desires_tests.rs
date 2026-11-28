//! The desires (blocks.md §3.5, §1.8, §5.1, §5.2) against B2's fold: every
//! history here is folded by `cards_spec::board`, so what a desk attaches,
//! settles and draws is checked against what the fold reads. §8.2's D6
//! list for desires, §3.5's notice list, the file (its caps, a full file,
//! a restart, a crash mid-write, the sweep, one writer and a second process
//! in memory only), Shady's #103 findings, and a model of two desks whose
//! boards age apart.

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

/// The desires in `d`'s file, the launch's sweep run, as its owner. (A
/// store just dropped may still hold the lock a moment: a process another
/// test spawns then holds a copy of it until it execs.)
pub(crate) fn desires(d: &Path) -> Desires {
    let t = std::time::Instant::now();
    loop {
        let mut a = Desires::load(Some(d.join(FILE)));
        if a.owns_file() || t.elapsed() > std::time::Duration::from_secs(10) {
            flush(&mut a);
            return a;
        }
        drop(a);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// A second desk on `d`'s file, its owner alive: memory only.
fn second(d: &Path) -> Desires {
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

/// What `d`'s file has, read as a restart would (its owner alive or not).
pub(crate) fn in_file(d: &Path) -> Book {
    blockfiles::peek::<Book>(&d.join(FILE)).0
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
                    Landed::Wish { conv: c, id: i, done, .. } if c == conv && i == *id => Some(done),
                    _ => None,
                })
                .expect("its write landed");
            done.map(|d| match d {
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
        let out = self.compacted(conv, &state.generation(), state.visibility());
        flush(self);
        out
    }

    fn ride_now(&mut self, conv: &str, g: &Generation, shared: &Visibility, body: &str, how: Carry) -> Ride {
        match self.ride(conv, g, shared, body, how).expect("not refused") {
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
    assert_eq!(in_file(&d).len(), 0, "and dropped from the file");
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
    assert_eq!((b.len(), in_file(b.path().unwrap().parent().unwrap()).len()), (0, 0), "nothing kept");
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
    assert_eq!((a.len(), in_file(&d).len()), (0, 0), "the predecessor's desires dropped");
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
    drop(a);
    let mut a = desires(&d);
    assert!(a.dismissed(J, &g, &id("tests"), &h.vis()), "restart, undelivered: the file");
    assert!(!b.dismissed(J, &g, &id("tests"), &h.vis()), "another desk: not yet");
    let r = a.ride_now(J, &g, &h.vis(), "go", Carry::Submit);
    h.user(&r.text);
    h.ops(json!([{"op":"patch","id":"tests","set":{"text":"y"}}]));
    a.settle_now(J, &g, &h.vis(), &[]);
    let fresh = Board::fold(g, &h.events).view(&h.events);
    drop(a);
    for (what, desk) in [("restarted", &desires(&d)), ("another desk", &b)] {
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
    drop(a);
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
    assert_eq!(in_file(&d).len(), c, "every accepted one kept");
    let mut m = serde_json::Map::new();
    m.insert(big.clone(), json!({"d": 1, "v": cards_spec::block::message::VERSION_MAX, "s": 1}));
    let entry = Value::Object(m).to_string();
    assert!(entry.len() - 2 <= ENTRY_MAX, "{entry}: {} bytes", entry.len() - 2);
}

const BLOCK_ID_LEN: usize = 24;

/// `w` run on a thread of its own: the background task.
fn in_background(w: Write) -> Landing {
    std::thread::spawn(move || w.run()).join().unwrap()
}

/// The UI thread never touches the file: every call returns having
/// changed memory only. A wish draws as before (pending) and rides nothing
/// until the snapshot with it, run in the background, one at a time, lands.
#[test]
fn the_ui_thread_never_touches_the_file() {
    let h = H::with(&["tests", "plan"]);
    let g = generation();
    let d = dir("never-waits");
    let disk = blockfiles::on_this_thread();
    let mut a = Desires::load(Some(d.join(FILE)));
    assert!(a.owns_file());
    assert_eq!(a.dismiss(J, &g, &id("tests"), &h.vis()), Ok(Ask::Saving));
    assert_eq!(a.dismiss(J, &g, &id("plan"), &h.vis()), Ok(Ask::Saving));
    assert_eq!(a.dismiss(J, &g, &id("plan"), &h.vis()), Ok(Ask::Unchanged), "asked already");
    assert!(!a.dismissed(J, &g, &id("tests"), &h.vis()) && a.pending(J, &g, &id("tests")), "drawn as before, pending");
    assert!(a.riding(J, &g, &h.vis()).is_empty(), "nothing rides before it is written");
    let plain = Riding::Go(Ride { text: "go".into(), carrier: None, notices: Vec::new() });
    assert_eq!(a.ride(J, &g, &h.vis(), "go", Carry::Submit), Ok(plain), "nothing rides, nothing waits: it goes");
    a.settle(J, &g, &h.vis(), &[]);
    a.discard("another");
    a.compacted("a-third", &h.state().generation(), h.state().visibility());
    let w = a.next_write().expect("the launch's sweep");
    assert!(a.next_write().is_none(), "one out at a time");
    assert_eq!(blockfiles::on_this_thread(), disk, "no write on the UI thread");
    let mut landed = a.landed(in_background(w));
    let w = a.next_write().expect("memory, whole");
    landed.extend(a.landed(in_background(w)));
    assert!(a.next_write().is_none() && !a.writing(), "one snapshot carried both");
    assert_eq!(blockfiles::on_this_thread(), disk);
    let mut done = wish_done(&landed);
    done.sort_by_key(|d| format!("{d:?}"));
    assert_eq!(done, [Ok(Done::Wished(1)), Ok(Done::Wished(2))]);
    assert!(a.dismissed(J, &g, &id("plan"), &h.vis()) && !a.pending(J, &g, &id("plan")));
    assert_eq!(a.riding(J, &g, &h.vis()).len(), 2, "written: they ride");
    // A message carrying them waits for their sent marks, written in the
    // background.
    let Ok(Riding::Wait(t)) = a.ride(J, &g, &h.vis(), "now", Carry::Submit) else { panic!("marked first") };
    let w = a.next_write().expect("the marks");
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
    assert!(in_file(&d).slot(J, &generation_key(&g)).unwrap().values().all(|x| x.sent));
}

/// A write the disk refuses refuses what it carried: the wishes in it are
/// undone in memory (what drew before draws again, said on the block),
/// the messages in it go plain; nothing waits for it, and the next change
/// writes memory whole.
#[test]
fn a_failed_write_refuses_and_draws_what_was_before() {
    let h = H::with(&["tests", "plan", "other"]);
    let g = generation();
    let d = dir("fails");
    let mut a = desires(&d);
    a.dismiss_now(J, &g, &id("plan"), &h.vis()).unwrap();
    a.dismiss_now(J, &g, &id("other"), &h.vis()).unwrap();
    // The disk refuses: a folder where the file goes.
    std::fs::remove_file(d.join(FILE)).unwrap();
    std::fs::create_dir(d.join(FILE)).unwrap();
    assert_eq!(a.dismiss(J, &g, &id("tests"), &h.vis()), Ok(Ask::Saving));
    assert_eq!(a.show(J, &g, &id("plan"), &h.vis()), Ok(Ask::Saving), "a cancel, pending");
    let Ok(Riding::Wait(t)) = a.ride(J, &g, &h.vis(), "words", Carry::Submit) else { panic!("its mark first") };
    let landed = flush(&mut a);
    let why = match &wish_done(&landed)[..] {
        [Err(Refused(x)), Err(Refused(y))] if x == y && x.contains(FILE) => x.clone(),
        other => panic!("{other:?}"),
    };
    assert!(landed.contains(&Landed::Send { ticket: t, ride: Ride { text: "words".into(), carrier: None, notices: Vec::new() } }), "plain");
    assert!(!a.dismissed(J, &g, &id("tests"), &h.vis()) && a.dismissed(J, &g, &id("plan"), &h.vis()), "what drew before");
    assert!(!a.pending(J, &g, &id("tests")) && !a.pending(J, &g, &id("plan")) && !a.writing());
    assert_eq!(a.refused(J, &id("tests")), Some(why.as_str()), "said on the block");
    assert_eq!(
        a.riding(J, &g, &h.vis()),
        [(id("plan"), Seen::Dismissed, 1), (id("other"), Seen::Dismissed, 2)],
        "plan's is back; other's not in flight"
    );
    std::fs::remove_dir(d.join(FILE)).unwrap();
    assert!(matches!(a.dismiss_now(J, &g, &id("tests"), &h.vis()), Ok(Now::Wished(_))), "the disk back: written");
    assert_eq!(a.refused(J, &id("tests")), None);
    drop(a);
    assert_eq!(desires(&d).len(), 3, "memory whole");
}

/// A write that failed isn't retried on its own, but a message that
/// waits for it, and a settle, try it again: the disk back, the notice
/// rides the next message, and the file has what memory has.
#[test]
fn a_failed_write_is_tried_again_by_a_message_and_a_settle() {
    let h = H::with(&["tests"]);
    let g = generation();
    let d = dir("retried");
    let mut a = desires(&d);
    a.dismiss_now(J, &g, &id("tests"), &h.vis()).unwrap();
    std::fs::remove_file(d.join(FILE)).unwrap();
    std::fs::create_dir(d.join(FILE)).unwrap();
    assert_eq!(a.ride_now(J, &g, &h.vis(), "one", Carry::Submit).text, "one", "its mark not written: plain");
    assert!(!a.writing(), "not retried on its own");
    std::fs::remove_dir(d.join(FILE)).unwrap();
    let r = a.ride_now(J, &g, &h.vis(), "two", Carry::Submit);
    assert_eq!(r.text, "two\n[block tests v1 dismissed]", "a message waiting for the file tries it again");
    drop(a);
    assert!(desires(&d).desire(J, &g, &id("tests")).is_some_and(|d| d.sent));

    let d = dir("retried-settle");
    let mut a = desires(&d);
    a.dismiss_now(J, &g, &id("tests"), &h.vis()).unwrap();
    std::fs::remove_file(d.join(FILE)).unwrap();
    std::fs::create_dir(d.join(FILE)).unwrap();
    assert_eq!(a.ride_now(J, &g, &h.vis(), "one", Carry::Submit).text, "one");
    std::fs::remove_dir(d.join(FILE)).unwrap();
    assert_eq!(a.settle_now(J, &g, &h.vis(), &[]), []);
    drop(a);
    assert!(desires(&d).desire(J, &g, &id("tests")).is_some_and(|d| d.sent), "a settle tried it again");
}

/// A write landing keeps what memory changed since: a desire settled
/// while the snapshot with it was out stays settled.
#[test]
fn a_landing_keeps_what_memory_changed_since() {
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
    assert!(a.desire(J, &g, &id("tests")).is_none(), "settled: dropped in memory");
    assert_eq!(a.dismiss(J, &g, &id("notes"), &h.vis()), Ok(Ask::Saving), "decided after that snapshot");
    let landed = a.landed(w.run());
    let notes = a.queued(J, &g, &id("notes")).expect("notes' write still queued");
    assert!(
        matches!(&landed[..], [Landed::Wish { conv, id: i, ticket, done: Ok(Done::Wished(2)) }] if conv == J && *i == id("plan") && *ticket != notes),
        "plan's only: {landed:?}"
    );
    assert!(a.desire(J, &g, &id("tests")).is_none(), "the plan's snapshot had tests: still dropped");
    assert!(a.desire(J, &g, &id("plan")).is_some());
    assert!(a.pending(J, &g, &id("notes")) && !a.dismissed(J, &g, &id("notes"), &h.vis()), "not in it: still pending");
    flush(&mut a);
    assert!(a.dismissed(J, &g, &id("notes"), &h.vis()));
    drop(a);
    assert_eq!(desires(&d).waiting(), [(J.to_string(), 2)]);
}

/// A wish on one not yet written is decided against it: dismiss then show
/// before either lands is a cancel (nothing kept, nothing rides), said
/// once, when it lands.
#[test]
fn a_wish_on_a_pending_one_is_decided_against_it() {
    let h = H::with(&["tests"]);
    let g = generation();
    let d = dir("pending-twice");
    let mut a = desires(&d);
    assert_eq!(a.dismiss(J, &g, &id("tests"), &h.vis()), Ok(Ask::Saving));
    assert_eq!(a.show(J, &g, &id("tests"), &h.vis()), Ok(Ask::Saving));
    assert_eq!(wish_done(&flush(&mut a)), [Ok(Done::Cancelled)]);
    assert!(!a.dismissed(J, &g, &id("tests"), &h.vis()) && a.len() == 0);
    assert_eq!(a.ride_now(J, &g, &h.vis(), "go", Carry::Submit).carrier, None, "nothing sent");
    assert_eq!(in_file(&d).len(), 0);
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
    drop(a);
    let g = generation_key(&generation());
    let text = json!({"version": 3, "conversations": {J: {g: {
        "tests": {"d": 1, "v": 1, "s": 0}, "Bad": {"d": 1, "v": 1, "s": 0}, "plan": {"d": 1, "v": 0, "s": 0}}}}});
    std::fs::write(d.join(FILE), text.to_string()).unwrap();
    assert_eq!(desires(&d).len(), 1);
}

/// A file of `desires` (conversation, id, sent), each dismissed at v1 in
/// [`generation`]'s generation, as a hand would write it.
fn hand_written(desires: impl IntoIterator<Item = (String, String, bool)>) -> String {
    let g = generation_key(&generation());
    let mut convs = serde_json::Map::new();
    for (conv, id, sent) in desires {
        let gens = convs.entry(conv).or_insert_with(|| json!({ g.as_str(): {} }));
        gens[g.as_str()][id] = json!({"d": 1, "v": 1, "s": u8::from(sent)});
    }
    json!({"version": 3, "conversations": convs}).to_string()
}

/// A hand-written file past the reserve (§1.8: no desk writes one) isn't
/// read: it reads as empty, said, and is moved aside (never copied: its
/// size is anyone's). One at the reserve is read; one byte more isn't.
#[test]
fn a_file_past_the_reserve_isnt_read() {
    let d = dir("oversized");
    let text = hand_written((0..3000).map(|i| (format!("c{i:04}"), format!("b{i:04}"), false)));
    assert!(text.len() > RESERVE, "{}", text.len());
    std::fs::write(d.join(FILE), &text).unwrap();
    let a = desires(&d);
    assert_eq!(a.len(), 0, "not read");
    assert!(a.trouble.as_deref().is_some_and(|t| t.contains(FILE) && t.contains("too big")), "said: {:?}", a.trouble);
    assert!(!d.join(FILE).exists(), "moved aside");
    assert_eq!(std::fs::metadata(d.join(format!("{FILE}.bad"))).unwrap().len(), text.len() as u64, "whole");
    drop(a);

    let one = hand_written([(J.to_string(), "tests".to_string(), false)]);
    for (pad, read) in [(RESERVE - one.len(), true), (RESERVE + 1 - one.len(), false)] {
        let d = dir(&format!("oversized-{pad}"));
        std::fs::write(d.join(FILE), format!("{one}{}", " ".repeat(pad))).unwrap();
        let a = desires(&d);
        assert_eq!((a.len(), a.trouble.is_none()), (usize::from(read), read), "{} bytes", one.len() + pad);
    }
}

/// A hand-written file past the counts (§1.8: no desk writes one) reads as
/// the writer would have admitted it, one desire at a time: those that may
/// have been sent first, then the file's order; [`PER_CONVERSATION`] and
/// [`PER_DESK`] at most. What's left out is said, and the file is copied
/// aside before anything is written over it.
#[test]
fn a_file_past_the_counts_reads_as_the_writer_admits_it() {
    let g = generation();
    let d = dir("past-the-counts");
    // 100 in one conversation, its last ten sent.
    let text = hand_written((0..100).map(|i| (J.to_string(), format!("b{i:03}"), i >= 90)));
    std::fs::write(d.join(FILE), &text).unwrap();
    let a = desires(&d);
    assert_eq!(a.len(), PER_CONVERSATION);
    let kept = |i: usize| a.desire(J, &g, &id(&format!("b{i:03}"))).is_some();
    assert!((90..100).all(kept), "the sent first");
    assert!((0..54).all(kept) && !(54..90).any(kept), "then the file's order");
    assert!(a.trouble.as_deref().is_some_and(|t| t.contains("36 entries past its caps")), "said: {:?}", a.trouble);
    assert_eq!(std::fs::read_to_string(d.join(format!("{FILE}.bad"))).unwrap(), text, "set aside");
    assert_eq!(std::fs::read_to_string(d.join(FILE)).unwrap(), text, "not written over yet");
    drop(a);
    assert_eq!(in_file(&d).len(), PER_CONVERSATION, "a restart reads the same");

    // 600 conversations, one each.
    let d = dir("past-the-desk");
    std::fs::write(d.join(FILE), hand_written((0..600).map(|i| (format!("c{i:03}"), "plan".to_string(), false)))).unwrap();
    let a = desires(&d);
    assert_eq!(a.len(), PER_DESK);
    assert!(a.desire("c511", &g, &id("plan")).is_some() && a.desire("c512", &g, &id("plan")).is_none(), "the file's order");
    assert!(a.trouble.as_deref().is_some_and(|t| t.contains("88 entries past its caps")), "said: {:?}", a.trouble);
}

/// What crashes left beside the file is swept by its owner's first write:
/// regular files of exactly `<file>.tmp.<pid>.<n>`, never a link (nor what
/// it names), a folder or another name.
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
/// replaced) count nowhere there, and go at the settle of another
/// generation (the history moved on), at a launch's first settle too; a
/// wish on the old one since is refused.
#[test]
fn another_generation_under_the_journal_drops_its_desires() {
    let h = H::with(&["tests"]);
    let (g, g2) = (generation(), Generation::of(&json!({"role":"user","content":"other"})));
    let d = dir("generation");
    let mut a = desires(&d);
    a.settle_now(J, &g, &h.vis(), &[]);
    a.dismiss_now(J, &g, &id("tests"), &h.vis()).unwrap();
    assert!(a.desire(J, &g2, &id("tests")).is_none() && !a.dismissed(J, &g2, &id("tests"), &h.vis()));
    assert_eq!(a.dismiss_now(J, &g2, &id("tests"), &h.vis()), Err(Refused(MOVED_ON.into())), "not this history's");
    a.settle_now(J, &g2, &h.vis(), &[]);
    assert_eq!((a.len(), in_file(&d).len()), (0, 0), "seen moving past: dropped");
    assert_eq!(a.dismiss_now(J, &g, &id("tests"), &h.vis()), Err(Refused(MOVED_ON.into())), "moved on");
    // A launch before its first settle keeps g's wish; that settle, at
    // g2, drops it.
    drop(a);
    let mut b = desires(&d);
    b.dismiss_now(J, &g, &id("tests"), &h.vis()).unwrap();
    drop(b);
    let mut c = desires(&d);
    assert_eq!(c.len(), 1);
    c.settle_now(J, &g2, &h.vis(), &[]);
    assert_eq!((c.len(), in_file(&d).len()), (0, 0), "c's first settle, on g2: g's go");
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

// One writer: processes.

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
    let h = H::with(&["c0"]);
    match what {
        "memory" => {
            // The parent owns the file: this desk keeps to memory, draws
            // and sends its wishes, and never touches the file.
            let before = std::fs::read(d.join(FILE)).unwrap();
            let mut a = second(&d);
            assert!(!a.owns_file() && a.trouble.as_deref().is_some_and(|t| t.contains("memory only")), "{:?}", a.trouble);
            assert_eq!(a.len(), 0, "the file isn't read");
            assert_eq!(a.dismiss_now("child", &generation(), &id("c0"), &h.vis()), Ok(Now::Wished(1)));
            assert!(a.dismissed("child", &generation(), &id("c0"), &h.vis()));
            let r = a.ride_now("child", &generation(), &h.vis(), "go", Carry::Submit);
            assert_eq!(line(&r), "[block c0 v1 dismissed]");
            assert_eq!(std::fs::read(d.join(FILE)).unwrap(), before, "never written");
            std::fs::write(d.join("done"), "").unwrap();
        }
        "crash" => {
            // The owner (its lock held), killed mid-write.
            let _owner = blockfiles::own(&d.join(FILE)).expect("the lock: no other desk");
            let _ = blockfiles::write_then(&d.join(FILE), "{}", || -> std::io::Result<()> { std::process::abort() });
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

/// One writer: a second desk process on the machine doesn't get the file's
/// lock, and keeps its desires in memory only (drawn, sent, never written;
/// said), as does a second store in this process; the owner goes on
/// writing. Once the owner is gone, the next launch owns the file.
#[test]
fn a_second_desk_keeps_its_desires_in_memory_only() {
    let d = dir("processes");
    let h = H::with(&["p0", "p1"]);
    let g = generation();
    let mut a = desires(&d);
    a.dismiss_now("parent", &g, &id("p0"), &h.vis()).unwrap();
    let out = spawn("memory", &d).wait_with_output().unwrap();
    assert!(out.status.success() && signalled(&d.join("done")), "{}", String::from_utf8_lossy(&out.stderr));
    let mut b = second(&d);
    assert!(!b.owns_file() && b.len() == 0);
    assert_eq!(b.dismiss_now("second", &g, &id("p0"), &h.vis()), Ok(Now::Wished(1)));
    assert!(b.next_write().is_none() && !b.writing(), "nothing to write: its memory is all");
    a.dismiss_now("parent", &g, &id("p1"), &h.vis()).unwrap();
    assert_eq!(in_file(&d).conversations.keys().collect::<Vec<_>>(), ["parent"], "the owner's only");
    drop((a, b));
    let c = desires(&d);
    assert!(c.owns_file() && c.trouble.is_none());
    assert_eq!(c.waiting(), [("parent".to_string(), 2)]);
}

/// The owner killed between writing its temporary file and the rename,
/// its lock held: the file is as it was, its lock went with it, and the
/// next launch owns the file and sweeps the leftover.
#[test]
fn a_crash_mid_write_keeps_the_file_and_the_next_load_sweeps() {
    let d = dir("crash");
    let mut a = desires(&d);
    a.dismiss_now(J, &generation(), &id("tests"), &H::default().vis()).unwrap();
    drop(a);
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
    let b = Desires::load(Some(d.join(FILE)));
    assert!(b.owns_file(), "the dead owner's lock is gone");
    drop(b);
    let b = desires(&d);
    assert_eq!((left(), b.waiting()), (0, vec![(J.to_string(), 1)]));
}

/// Shady #103 R3-P2-3: a write issued holds the file's lock until it ran,
/// its store dropped or not: no other store owns the file meanwhile. The
/// last write (a window's end) carries what memory changed behind the one
/// out; once both ran, the lock is free and the file has both.
#[test]
fn a_write_out_holds_the_lock_past_its_store() {
    let d = dir("lock-outlives");
    let h = H::with(&["a1", "a2"]);
    let g = generation();
    let mut a = desires(&d);
    assert_eq!(a.dismiss(J, &g, &id("a1"), &h.vis()), Ok(Ask::Saving));
    let w = a.next_write().expect("a1's");
    assert_eq!(a.dismiss(J, &g, &id("a2"), &h.vis()), Ok(Ask::Saving));
    assert!(a.next_write().is_none(), "one out: a2 behind it");
    let last = a.last_write().expect("memory whole");
    drop(a);
    let b = Desires::load(Some(d.join(FILE)));
    assert!(!b.owns_file(), "the writes out hold the lock");
    drop(b);
    drop(w.run());
    let b = Desires::load(Some(d.join(FILE)));
    assert!(!b.owns_file(), "the last write out alone holds it");
    drop(b);
    drop(last.run());
    let mut c = desires(&d);
    assert!(c.owns_file());
    assert_eq!(c.waiting(), [(J.to_string(), 2)], "both written");
    // A write out alone (no last one) holds it too.
    assert_eq!(c.show(J, &g, &id("a1"), &h.vis()), Ok(Ask::Saving));
    let w = c.next_write().expect("a1's show");
    drop(c);
    let b = Desires::load(Some(d.join(FILE)));
    assert!(!b.owns_file(), "the write out alone holds the lock");
    drop(b);
    drop(w.run());
    assert!(desires(&d).owns_file());
}

// Shady's review of #103, round 1 (bba9546).

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
            drop(a);
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

/// Settled feedback is bounded per conversation and in all, goes once
/// seen, and goes with its generation and its conversation, desires or
/// none (P2-5).
#[test]
fn settled_feedback_is_bounded_and_goes() {
    let g = generation();
    let mut a = desires(&dir("feedback"));
    let later = std::cell::Cell::new(1_000);
    let lose = |a: &mut Desires, conv: &str, prefix: &str| {
        let mut h = H::default();
        for i in 0..PER_CONVERSATION {
            a.dismiss_now(conv, &g, &id(&format!("{prefix}{i}")), &h.vis()).unwrap();
        }
        // Another desk's later Shows of every one of them: above all this
        // desk has minted or settled.
        later.set(later.get() + 1_000);
        others(&mut h, prefix, PER_CONVERSATION, later.get(), "shown");
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

/// A message carries only the notices memory still has so when its turn
/// comes: a Show supersedes the dismissal it picked while it waits for
/// the file, so it goes plain, and the Show (an obligation, its sent mark
/// inherited) rides the next message.
#[test]
fn a_send_carries_only_what_memory_still_has() {
    let h = H::with(&["tests", "plan"]);
    let g = generation();
    let mut a = desires(&dir("send-checks-memory"));
    a.dismiss_now(J, &g, &id("tests"), &h.vis()).unwrap();
    let first = a.ride_now(J, &g, &h.vis(), "first", Carry::Steer);
    a.lost(first.carrier.unwrap());
    assert_eq!(a.dismiss(J, &g, &id("plan"), &h.vis()), Ok(Ask::Saving), "the file behind memory");
    let Ok(Riding::Wait(t)) = a.ride(J, &g, &h.vis(), "second", Carry::Submit) else { panic!("waits for the file") };
    assert_eq!(a.show(J, &g, &id("tests"), &h.vis()), Ok(Ask::Saving));
    let sent = flush(&mut a).into_iter().find_map(|l| match l {
        Landed::Send { ticket, ride } if ticket == t => Some(ride),
        _ => None,
    });
    let sent = sent.expect("its turn came");
    assert_eq!((sent.text.as_str(), sent.carrier), ("second", None), "v1 is superseded: nothing rides");
    let next = a.ride_now(J, &g, &h.vis(), "third", Carry::Submit);
    assert_eq!(line(&next), "[block tests v3 shown]");
    assert!(next.text.contains("[block plan v2 dismissed]"));
}

// Shady's review of #103, round 2 (22921cf): each of these fails there
// (d6b-evidence/failfirst/r2), but R2-P2-4's (presentation_tests.rs).

/// R2-P2-1, on one desk (two desks no longer share a file: the round-2
/// case, one desk's file settling the other's notices, can't arise): its
/// notices land out of order (v2, then v1); it settles both; then the
/// user clicks on a board older than that settle, with the same highest
/// (v2 seen, v1 not), dismiss then show. The Show must hold: what the
/// desk settled draws the id dismissed, so the click's board doesn't
/// decide a cancel, and the Show is a notice.
#[test]
fn a_board_older_than_the_desks_settle_decides_no_cancel() {
    let h0 = H::with(&["tests", "other"]);
    let g = generation();
    let mut a = desires(&dir("older-board"));
    a.dismiss_now(J, &g, &id("tests"), &h0.vis()).unwrap();
    let first = a.ride_now(J, &g, &h0.vis(), "one", Carry::Submit);
    a.dismiss_now(J, &g, &id("other"), &h0.vis()).unwrap();
    let second = a.ride_now(J, &g, &h0.vis(), "two", Carry::Submit);
    let mut h = h0.clone();
    h.user(&second.text);
    let older = h.vis();
    h.user(&first.text);
    assert!(h.vis().dismissed(&id("tests")) && h.vis().highest() == 2 && older.highest() == 2);
    a.settle_now(J, &g, &h.vis(), &[]);
    assert_eq!(a.len(), 0);
    assert!(!a.dismissed(J, &g, &id("tests"), &older), "the older board draws it shown");
    assert_eq!(a.dismiss_now(J, &g, &id("tests"), &older), Ok(Now::Wished(3)));
    assert_eq!(a.show_now(J, &g, &id("tests"), &older), Ok(Now::Wished(4)), "a notice, not a cancel");
    let next = a.ride_now(J, &g, &older, "three", Carry::Submit);
    assert_eq!(line(&next), "[block tests v4 shown]");
    h.user(&next.text);
    assert!(!h.vis().dismissed(&id("tests")), "the Show holds");
    // On a board as new as the settle, an unsent dismissal is simply
    // cancelled (§3.5).
    assert_eq!(a.settle_now(J, &g, &h.vis(), &[]), []);
    assert_eq!(a.dismiss_now(J, &g, &id("other"), &h.vis()), Ok(Now::Unchanged), "already dismissed");
    assert_eq!(a.show_now(J, &g, &id("other"), &h.vis()), Ok(Now::Wished(5)));
    assert_eq!(a.dismiss_now(J, &g, &id("other"), &h.vis()), Ok(Now::Cancelled));
}

/// An unsent dismissal over a board that now draws the block dismissed
/// (another desk's notice landed): Show is a new version, not a cancel,
/// which would draw the shared state and lose the Show; and on a board
/// that draws it shown, the cancel (§3.5).
#[test]
fn an_unsent_desire_the_board_now_draws_is_superseded() {
    let h0 = H::with(&["tests"]);
    let g = generation();
    let mut a = desires(&dir("unsent-board"));
    a.dismiss_now(J, &g, &id("tests"), &h0.vis()).unwrap();
    let mut h = h0.clone();
    h.user("theirs\n[block tests v5 dismissed]");
    assert_eq!(a.show_now(J, &g, &id("tests"), &h.vis()), Ok(Now::Wished(6)));
    let r = a.ride_now(J, &g, &h.vis(), "go", Carry::Submit);
    h.user(&r.text);
    assert!(!h.vis().dismissed(&id("tests")), "the Show holds");
    assert_eq!(a.settle_now(J, &g, &h.vis(), &[]), []);
    assert_eq!(a.dismiss_now(J, &g, &id("tests"), &h.vis()), Ok(Now::Wished(7)));
    assert_eq!(a.show_now(J, &g, &id("tests"), &h.vis()), Ok(Now::Cancelled), "unsent, and every board draws it shown");
}

/// What this process knows of a conversation's history is bounded
/// ([`KNOWN_MAX`] conversations without desires), and never forgotten for
/// one with desires: there, a board older than its settle still decides
/// no cancel (R2-P2-1's case) after 300 other conversations settled.
#[test]
fn what_is_known_is_bounded_but_kept_where_desires_wait() {
    let h0 = H::with(&["tests", "other", "plan"]);
    let g = generation();
    let mut a = desires(&dir("known"));
    a.dismiss_now(J, &g, &id("tests"), &h0.vis()).unwrap();
    let first = a.ride_now(J, &g, &h0.vis(), "one", Carry::Submit);
    a.dismiss_now(J, &g, &id("other"), &h0.vis()).unwrap();
    let second = a.ride_now(J, &g, &h0.vis(), "two", Carry::Submit);
    a.hold(J, &g, &id("plan"), &h0.vis());
    let mut h = h0.clone();
    h.user(&second.text);
    let older = h.vis();
    h.user(&first.text);
    a.settle_now(J, &g, &h.vis(), &[]);
    a.dismiss_now(J, &g, &id("plan"), &h.vis()).unwrap();
    assert_eq!(a.len(), 1, "plan waits");
    for c in 0..300 {
        a.settle_now(&format!("{J}-{c}"), &g, &h0.vis(), &[]);
    }
    assert!(a.known.len() <= KNOWN_MAX + 1, "{}", a.known.len());
    assert_eq!(a.dismiss_now(J, &g, &id("tests"), &older), Ok(Now::Wished(4)));
    assert_eq!(a.show_now(J, &g, &id("tests"), &older), Ok(Now::Wished(5)), "a notice, not a cancel");
}

/// R2-P2-2: a version known settled is never minted under. A typed
/// `shown` at v100 settles this desk's v1; a Dismiss on a board older
/// than it (highest 0) mints v101, and holds.
#[test]
fn minting_goes_above_a_settled_typed_version() {
    let h0 = H::with(&["tests"]);
    let g = generation();
    let mut a = desires(&dir("typed-v100"));
    a.dismiss_now(J, &g, &id("tests"), &h0.vis()).unwrap();
    let r = a.ride_now(J, &g, &h0.vis(), "one", Carry::Submit);
    let mut h = h0.clone();
    h.user(&r.text);
    h.user("typed\n[block tests v100 shown]");
    assert_eq!(a.settle_now(J, &g, &h.vis(), &[]), [(id("tests"), Feedback::ChangedByLater)]);
    assert_eq!(a.dismiss_now(J, &g, &id("tests"), &h0.vis()), Ok(Now::Wished(101)));
    let next = a.ride_now(J, &g, &h0.vis(), "two", Carry::Submit);
    h.user(&next.text);
    assert!(h.vis().dismissed(&id("tests")), "the dismissal holds");
}

/// R2-P2-3: what may still land is never forgotten. The user's discard
/// drops unsent desires only: a dismissal sent (its carrier delayed) stays,
/// an obligation, through other conversations' churn (by count and by
/// bytes) and a restart; the user's Show supersedes it, and holds
/// whichever lands first.
#[test]
fn a_discard_keeps_what_may_still_land() {
    let h0 = H::with(&["tests", "x"]);
    let g = generation();
    for (churn, restart) in [("count", false), ("bytes", false), ("count", true)] {
        let d = dir(&format!("discard-{churn}-{restart}"));
        let mut a = desires(&d);
        a.dismiss_now(J, &g, &id("tests"), &h0.vis()).unwrap();
        let first = a.ride_now(J, &g, &h0.vis(), "also", Carry::Steer);
        assert_eq!(line(&first), "[block tests v1 dismissed]");
        a.dismiss_now(J, &g, &id("x"), &h0.vis()).unwrap();
        assert_eq!(a.discard(J), 1, "the unsent one");
        assert!(a.desire(J, &g, &id("tests")).is_some_and(|d| d.sent) && a.desire(J, &g, &id("x")).is_none());
        let (n, key) = if churn == "count" { (65, 0) } else { (300, KEY_MAX - 4) };
        for i in 0..n {
            let k = format!("{i:04}{}", "k".repeat(key));
            a.dismiss_now(&k, &g, &id("x"), &h0.vis()).unwrap();
            assert_eq!(a.show_now(&k, &g, &id("x"), &h0.vis()), Ok(Now::Cancelled));
        }
        if restart {
            drop(a);
            a = desires(&d);
        }
        assert!(a.dismissed(J, &g, &id("tests"), &h0.vis()), "{churn} {restart}: still this desk's wish");
        assert!(matches!(a.show_now(J, &g, &id("tests"), &h0.vis()), Ok(Now::Wished(v)) if v > 1), "never a cancel");
        let next = a.ride_now(J, &g, &h0.vis(), "go", Carry::Submit);
        assert!(line(&next).ends_with("shown]"), "{}", next.text);
        for order in [[&first, &next], [&next, &first]] {
            let mut h = h0.clone();
            order.iter().for_each(|r| h.user(&r.text));
            assert!(!h.vis().dismissed(&id("tests")), "{churn} {restart}: the Show holds");
        }
    }
}

/// R2-P2-5: a generation switch (or a compaction) while the write of a
/// wish of the old one is out: once it lands, no desire of the old
/// generation stays (memory, file, restart), and the retired wish isn't
/// reported (a section of the new history may have its id).
#[test]
fn a_wish_out_across_a_generation_switch_is_retired() {
    let h = H::with(&["tests"]);
    let (g1, g2) = (generation(), Generation::of(&json!({"role":"user","content":"another start"})));
    for how in ["switch", "compaction"] {
        let d = dir(&format!("retired-{how}"));
        let mut a = desires(&d);
        a.settle_now(J, &g1, &h.vis(), &[]);
        assert_eq!(a.dismiss(J, &g1, &id("tests"), &h.vis()), Ok(Ask::Saving));
        let w = a.next_write().expect("its write");
        if how == "switch" {
            a.settle(J, &g2, &h.vis(), &[]);
        } else {
            let s = h.state();
            assert_eq!(a.compacted(J, &s.generation(), s.visibility()), [], "never drawn dismissed");
        }
        assert!(!a.pending(J, &g1, &id("tests")));
        let mut landed = a.landed(in_background(w));
        landed.extend(flush(&mut a));
        assert_eq!(wish_done(&landed), [], "{how}: retired, never reported");
        assert_eq!((a.len(), in_file(&d).len()), (0, 0), "{how}");
        drop(a);
        let mut b = desires(&d);
        assert_eq!(b.len(), 0, "{how}: a restart");
        b.settle_now(J, &g2, &h.vis(), &[]);
        assert_eq!(b.len(), 0);
    }
}

/// Each queued write has a ticket of its own (Shady #106, round 3): the
/// same through every wish on its id until it lands (its landing names
/// it, and lands it alone, never a wish on the id since), gone once it is
/// retired (another generation's settle, a compaction, a discard), and
/// never given again.
#[test]
fn a_queued_write_keeps_its_ticket_until_it_lands_or_is_retired() {
    let h = H::with(&["tests", "plan"]);
    let (g1, g2) = (generation(), Generation::of(&json!({"role":"user","content":"another start"})));
    let d = dir("tickets");
    let mut a = desires(&d);
    a.settle_now(J, &g1, &h.vis(), &[]);
    assert_eq!(a.dismiss(J, &g1, &id("tests"), &h.vis()), Ok(Ask::Saving));
    let t1 = a.queued(J, &g1, &id("tests")).expect("queued");
    assert_eq!(a.show(J, &g1, &id("tests"), &h.vis()), Ok(Ask::Saving));
    assert_eq!(a.queued(J, &g1, &id("tests")), Some(t1), "the same write");
    assert_eq!(a.dismiss(J, &g1, &id("plan"), &h.vis()), Ok(Ask::Saving));
    let t2 = a.queued(J, &g1, &id("plan")).expect("queued");
    assert_ne!(t1, t2, "a write each");
    let w = a.next_write().expect("its write");
    assert_eq!(a.dismiss(J, &g1, &id("tests"), &h.vis()), Ok(Ask::Saving), "after that snapshot");
    let tickets =
        |l: &[Landed]| l.iter().filter_map(|l| if let Landed::Wish { ticket, .. } = l { Some(*ticket) } else { None }).collect::<Vec<_>>();
    assert_eq!(tickets(&a.landed(w.run())), [t2], "plan's alone");
    assert_eq!(a.queued(J, &g1, &id("tests")), Some(t1), "tests' still waits, its own");
    let mut seen = vec![t1, t2];
    for how in ["switch", "compaction", "discard"] {
        let g = if how == "switch" { &g1 } else { &g2 };
        if how != "switch" {
            assert_eq!(a.dismiss(J, g, &id("tests"), &h.vis()), Ok(Ask::Saving), "{how}");
        }
        let t = a.queued(J, g, &id("tests")).expect("queued");
        assert!(how == "switch" || !seen.contains(&t), "{how}: never given again");
        seen.push(t);
        match how {
            "switch" => _ = a.settle(J, &g2, &h.vis(), &[]),
            "compaction" => _ = a.compacted(J, &g2, &h.vis()),
            _ => _ = a.discard(J),
        }
        assert_eq!((a.queued(J, g, &id("tests")), a.queued_wishes()), (None, 0), "{how}: retired");
        assert!(tickets(&flush(&mut a)).is_empty(), "{how}: never landed");
    }
}

/// R2-P2-6: every wait is bounded. With the first message's write held
/// (the background task stuck), messages queue behind it, and wishes wait
/// for the file, interleaved with settles; past QUEUE_MAX in all (or
/// QUEUE_BYTES_MAX of messages) the newest is refused, said, and nothing
/// is dropped. Let go, every accepted message goes, in order.
#[test]
fn every_wait_is_bounded_and_the_newest_is_refused() {
    let h = H::with(&["tests"]);
    let g = generation();
    let mut a = desires(&dir("bounded"));
    a.dismiss_now(J, &g, &id("tests"), &h.vis()).unwrap();
    let Ok(Riding::Wait(first)) = a.ride(J, &g, &h.vis(), "first", Carry::Submit) else { panic!("its mark first") };
    let held = a.next_write().expect("the mark's write, held");
    assert_eq!(a.ride(J, &g, &h.vis(), &"x".repeat(QUEUE_BYTES_MAX), Carry::Submit), Err(Refused(BUSY.into())), "bytes");
    let (mut queued, mut wishes, mut refused) = (vec![first], 0, 0);
    for i in 0..1000 {
        match a.ride(J, &g, &h.vis(), &format!("m{i}"), Carry::Submit) {
            Ok(Riding::Wait(t)) => queued.push(t),
            Ok(Riding::Go(r)) => panic!("{r:?} went ahead of the first"),
            Err(Refused(why)) => {
                assert_eq!(why, BUSY);
                refused += 1;
            }
        }
        a.settle(J, &g, &h.vis(), &[]);
        if i % 10 == 0 {
            match a.dismiss(J, &g, &id(&format!("w{i}")), &h.vis()) {
                Ok(Ask::Saving) => wishes += 1,
                Err(Refused(why)) => assert_eq!(why, BUSY),
                other => panic!("{other:?}"),
            }
        }
        assert!(a.waiting_actions() <= QUEUE_MAX, "{i}: {}", a.waiting_actions());
    }
    assert_eq!(queued.len() + wishes, QUEUE_MAX);
    assert!(refused > 900);
    let mut landed = a.landed(in_background(held));
    landed.extend(flush(&mut a));
    let sent: Vec<&Ride> = landed.iter().filter_map(|l| if let Landed::Send { ride, .. } = l { Some(ride) } else { None }).collect();
    let order: Vec<u64> = landed.iter().filter_map(|l| if let Landed::Send { ticket, .. } = l { Some(*ticket) } else { None }).collect();
    assert_eq!(order, queued, "every accepted one, in order");
    assert_eq!(sent[0].text, "first\n[block tests v1 dismissed]");
    assert_eq!(wish_done(&landed).len(), wishes);
    assert_eq!(a.waiting_actions(), 0);
}

/// R2-P2-7: every transition keeps the file within its reserve. A file
/// just under it (one 128-byte journal with 64 generations of one unsent
/// desire, other journals of one), each of the 64 cancelled, the rest
/// discarded: the file never grows past 64 KiB (at 22921cf each cancel
/// left a mark repeating the 128-byte key: 65,578 bytes after 53).
#[test]
fn every_transition_keeps_the_reserve() {
    let d = dir("reserve-every");
    let big = "k".repeat(KEY_MAX);
    let gens: Vec<Generation> = (0..64).map(|i| Generation::of(&json!({"role":"user","content":format!("start {i}")}))).collect();
    let mut book = Book::default();
    let one = |conv: &str, g: &Generation| (conv.to_string(), generation_key(g), id("a"), Some(Desire::new(true, VERSION_MAX, false)));
    for g in &gens {
        let (c, k, i, x) = one(&big, g);
        book.set(&c, &k, &i, x);
    }
    let mut n = 0;
    loop {
        let mut next = book.clone();
        let (c, k, i, x) = one(&format!("{}-{n:04}", "j".repeat(100)), &generation());
        next.set(&c, &k, &i, x);
        if next.widest_bytes() > RESERVE - 600 {
            break;
        }
        book = next;
        n += 1;
    }
    assert!(book.widest_bytes() <= RESERVE && book.len() <= PER_DESK, "{n}");
    std::fs::write(d.join(FILE), book.text()).unwrap();
    let mut a = desires(&d);
    assert_eq!(a.len(), 64 + n);
    let shown = H::with(&["a"]).vis();
    let mut last = book.text().len();
    for (i, g) in gens.iter().enumerate() {
        assert_eq!(a.show_now(&big, g, &id("a"), &shown), Ok(Now::Cancelled));
        let bytes = std::fs::read(d.join(FILE)).unwrap().len();
        assert!(bytes < last, "after {} cancels: {bytes} bytes, from {last}", i + 1);
        last = bytes;
    }
    for i in 0..n {
        assert_eq!(a.discard(&format!("{}-{i:04}", "j".repeat(100))), 1);
        flush(&mut a);
        let bytes = std::fs::read(d.join(FILE)).unwrap().len();
        assert!(bytes < last, "after {} discards: {bytes} bytes", i + 1);
        last = bytes;
    }
    assert_eq!((a.len(), in_file(&d).len()), (0, 0));
}

/// A file made exactly `target` bytes long, of one-desire conversations
/// keyed to fit: each desire dismissed at `v`, sent.
fn edge_book(target: usize, v: u32) -> Book {
    let gk = generation_key(&generation());
    let t = id(&format!("{:x<24}", "t"));
    let add = |b: &mut Book, key: &str| b.set(key, &gk, &t, Some(Desire::new(true, v, true)));
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
    book
}

/// The reserve on the file as written, version widths counted: a file
/// exactly at 64 KiB refuses a Show of one of its desires that would
/// write v10 over v9 (a byte more): refused at once, nothing drawn,
/// nothing written. A new id is admitted only if every desire could still
/// be superseded at its widest version.
#[test]
fn the_reserve_counts_a_versions_width() {
    let g = generation();
    let d = dir("reserve-width");
    let book = edge_book(RESERVE, 9);
    std::fs::write(d.join(FILE), book.text()).unwrap();
    let mut a = desires(&d);
    assert_eq!(a.len(), book.len());
    let (conv, slot) = book.conversations.iter().next().map(|(c, g)| (c.clone(), g.values().next().unwrap().clone())).unwrap();
    let t = slot.keys().next().unwrap().clone();
    let shared = H::with(&[t.as_str()]).vis();
    assert!(a.dismissed(&conv, &g, &t, &shared));
    let refused = a.show(&conv, &g, &t, &shared);
    assert!(matches!(&refused, Err(Refused(why)) if why.starts_with(FULL_DESK) && why.contains("65537 bytes")), "{refused:?}");
    assert!(a.dismissed(&conv, &g, &t, &shared) && !a.pending(&conv, &g, &t) && !a.writing(), "nothing drawn, nothing to write");
    assert_eq!(std::fs::read_to_string(d.join(FILE)).unwrap(), book.text(), "nothing written");

    // Admission at the widest: a book whose desires fit now but not at
    // their widest takes no new id; one that fits at the widest takes it,
    // and every desire can then go to VERSION_MAX.
    let d = dir("reserve-widest");
    let narrow = edge_book(RESERVE - 1_000, 1);
    assert!(narrow.widest_bytes() > RESERVE && narrow.text().len() < RESERVE);
    std::fs::write(d.join(FILE), narrow.text()).unwrap();
    let mut a = desires(&d);
    let refused = a.dismiss(J, &g, &id("new"), &H::default().vis());
    assert!(matches!(&refused, Err(Refused(why)) if why.contains("at the widest")), "{refused:?}");
    let mut fits = edge_book(RESERVE - 1_000, 1);
    while fits.widest_bytes() + 200 > RESERVE {
        let k = fits.conversations.keys().next().unwrap().clone();
        fits.conversations.remove(&k);
    }
    let d = dir("reserve-fits");
    std::fs::write(d.join(FILE), fits.text()).unwrap();
    let mut a = desires(&d);
    assert_eq!(a.dismiss_now(J, &g, &id("new"), &H::default().vis()), Ok(Now::Wished(1)));
    assert!(in_file(&d).widest_bytes() <= RESERVE, "every admitted desire can be superseded");
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

/// One desk of the model: its store, its board (how much of the history it
/// has seen, and as of its launch), its write out, the wishes it has asked
/// and not yet written, and the highest version it was shown since launch.
struct Desk {
    store: Desires,
    dir: PathBuf,
    view: usize,
    launched: usize,
    out: Option<Write>,
    asked: BTreeMap<BlockId, bool>,
    seen: u32,
}

/// A model of two desks, each with its file, on one conversation's
/// history, and a third party typing notices on the id both share. Each
/// desk's board ages apart: it sees the history late, and clicks (on the
/// opposite of what it draws) and sends against any board it has had since
/// its launch, the newest or older ones. Writes land at once or later (a
/// worker slow to finish), or fail; messages are delivered in order, late,
/// out of order or dropped; desks settle and restart. Every version a desk
/// mints is above every one it was shown since its launch. Once
/// everything is written, sent, delivered and settled, nothing waits, and
/// each id draws on both desks what the history has; for an id only one
/// desk changes, its last wish written.
#[test]
fn two_desks_whose_boards_age_apart_end_as_their_last_wishes() {
    let names = ["a0", "a1", "b0", "b1", "e"];
    let owner = |n: &str| n.as_bytes()[0];
    let g = generation();
    let gk = generation_key(&g);
    // `SYNAPS_D6B_SEEDS` runs more (a deeper look); 40 by default.
    let seeds = std::env::var("SYNAPS_D6B_SEEDS").ok().and_then(|n| n.parse().ok()).unwrap_or(40u64);
    let first = std::env::var("SYNAPS_D6B_SEED").ok().and_then(|n| n.parse().ok()).unwrap_or(1u64);
    let steps = std::env::var("SYNAPS_D6B_STEPS").ok().and_then(|n| n.parse().ok()).unwrap_or(60usize);
    for seed in first..=seeds.max(first) {
        let root = dir(&format!("model-{seed}"));
        let bad = Some(root.join("not-a-folder").join(FILE));
        std::fs::write(root.join("not-a-folder"), "x").unwrap();
        let mut dice = Dice(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let mut h = H::with(&names);
        let start = h.events.len();
        let mut desks: Vec<Desk> = (0..2)
            .map(|k| {
                let d = root.join(format!("desk{k}"));
                std::fs::create_dir_all(&d).unwrap();
                Desk { store: desires(&d), dir: d, view: start, launched: start, out: None, asked: BTreeMap::new(), seen: 0 }
            })
            .collect();
        let mut want: BTreeMap<BlockId, bool> = BTreeMap::new();
        let mut net: Vec<(usize, Ride)> = Vec::new();
        let mut trace = Vec::new();
        let board = |h: &H, n: usize| H { events: h.events[..n].to_vec(), tools: 0 }.vis();
        // A desk's writes: the one out (failing or not), then every one to
        // make, at once. What landed: wishes kept or not, messages sent.
        let land = |desk: &mut Desk, fail: bool, want: &mut BTreeMap<BlockId, bool>, trace: &mut Vec<String>| {
            let mut rides = Vec::new();
            while let Some(mut w) = desk.out.take().or_else(|| desk.store.next_write()) {
                if fail {
                    w.path.clone_from(&bad);
                }
                for l in desk.store.landed(w.run()) {
                    match l {
                        Landed::Wish { id, done, .. } => {
                            let dismiss = desk.asked.remove(&id).expect("asked");
                            trace.push(format!(
                                "  landed {id} {}: {done:?}{}",
                                if dismiss { "x" } else { "u" },
                                if fail { " (failing)" } else { "" }
                            ));
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
                let bytes = std::fs::metadata(desk.dir.join(FILE)).map_or(0, |m| m.len() as usize);
                assert!(bytes <= RESERVE, "{bytes}");
            }
            rides
        };
        for _ in 0..steps {
            let k = dice.below(2);
            let aged = |dice: &mut Dice, desk: &Desk| desk.launched + dice.below(desk.view - desk.launched + 1);
            match dice.below(16) {
                0..=4 => {
                    let mine: Vec<&str> = names.iter().copied().filter(|n| owner(n) == b"ab"[k] || owner(n) == b'e').collect();
                    let i = id(mine[dice.below(mine.len())]);
                    let at = aged(&mut dice, &desks[k]);
                    let v = board(&h, at);
                    let desk = &mut desks[k];
                    let dismiss = !desk.store.dismissed(J, &g, &i, &v);
                    let floor = desk.seen.max(v.highest());
                    let was = desk.store.book.get(J, &gk, &i).map(|x| x.version);
                    let r = if dismiss { desk.store.dismiss(J, &g, &i, &v) } else { desk.store.show(J, &g, &i, &v) };
                    trace.push(format!("{k} {} {i} on {at}/{}: {r:?}", if dismiss { "x" } else { "u" }, h.events.len()));
                    if r == Ok(Ask::Saving) {
                        desk.asked.insert(i.clone(), dismiss);
                        let now = desk.store.book.get(J, &gk, &i).map(|x| x.version);
                        if let Some(n) = now.filter(|n| Some(*n) != was) {
                            assert!(n > floor, "seed {seed}: minted v{n}, shown v{floor}\n{}", trace.join("\n"));
                        }
                    }
                    desk.seen = floor.max(desk.store.book.get(J, &gk, &i).map_or(0, |x| x.version));
                    if dice.below(3) > 0 {
                        let fail = dice.below(6) == 0;
                        let rides = land(&mut desks[k], fail, &mut want, &mut trace);
                        net.extend(rides.into_iter().map(|r| (k, r)));
                    }
                }
                5..=6 => {
                    let at = aged(&mut dice, &desks[k]);
                    let v = board(&h, at);
                    let body = format!("m{}", trace.len());
                    desks[k].seen = desks[k].seen.max(v.highest());
                    match desks[k].store.ride(J, &g, &v, &body, Carry::Submit) {
                        Ok(Riding::Go(r)) => {
                            trace.push(format!("{k} sends {:?} on {at}", r.text));
                            net.push((k, r));
                        }
                        Ok(Riding::Wait(_)) => {
                            trace.push(format!("{k} waits {body:?} on {at}"));
                            let rides = land(&mut desks[k], false, &mut want, &mut trace);
                            net.extend(rides.into_iter().map(|r| (k, r)));
                        }
                        Err(e) => trace.push(format!("{k} refused {body:?}: {e:?}")),
                    }
                }
                7..=8 if !net.is_empty() => {
                    let (from, r) = net.remove(dice.below(net.len()));
                    if dice.below(5) == 0 {
                        trace.push(format!("dropped {:?}", r.text));
                        if let Some(c) = r.carrier {
                            desks[from].store.lost(c);
                        }
                    } else {
                        trace.push(format!("delivered {:?}", r.text));
                        h.user(&r.text);
                    }
                }
                9 => {
                    let desk = &mut desks[k];
                    desk.view += dice.below(h.events.len() - desk.view + 1);
                }
                10..=11 => {
                    let v = board(&h, desks[k].view);
                    trace.push(format!("{k} settles on {}", desks[k].view));
                    let flying: Vec<u64> = net.iter().filter(|(f, _)| *f == k).filter_map(|(_, r)| r.carrier).collect();
                    desks[k].seen = desks[k].seen.max(v.highest());
                    desks[k].store.settle(J, &g, &v, &flying);
                    if dice.below(2) == 0 {
                        let rides = land(&mut desks[k], false, &mut want, &mut trace);
                        net.extend(rides.into_iter().map(|r| (k, r)));
                    }
                }
                12 => {
                    // A third party types a notice on the shared id.
                    let n = 1 + dice.below(board(&h, h.events.len()).highest() as usize + 2);
                    let kind = if dice.below(2) == 0 { "dismissed" } else { "shown" };
                    trace.push(format!("typed e v{n} {kind}"));
                    h.user(&format!("typed\n[block e v{n} {kind}]"));
                }
                13 => {
                    // The worker takes a write and is slow to finish it.
                    if desks[k].out.is_none() {
                        desks[k].out = desks[k].store.next_write();
                        trace.push(format!("{k} write out: {}", desks[k].out.is_some()));
                    }
                }
                14 => {
                    let fail = dice.below(4) == 0;
                    trace.push(format!("{k} lands{}", if fail { " (failing)" } else { "" }));
                    let rides = land(&mut desks[k], fail, &mut want, &mut trace);
                    net.extend(rides.into_iter().map(|r| (k, r)));
                }
                15 => {
                    trace.push(format!("{k} restarts"));
                    let desk = &mut desks[k];
                    desk.out = None;
                    desk.asked.clear();
                    let d = desk.dir.clone();
                    drop(std::mem::replace(&mut desk.store, Desires::load(None)));
                    desk.store = desires(&d);
                    // It attaches to the history as it is now.
                    (desk.view, desk.launched, desk.seen) = (h.events.len(), h.events.len(), 0);
                    // Its messages already sent are the network's now.
                    for (f, r) in &mut net {
                        if *f == k {
                            r.carrier = None;
                        }
                    }
                }
                _ => {}
            }
        }
        // Everything written, sent, delivered and settled.
        for round in 0..20 {
            for (k, desk) in desks.iter_mut().enumerate() {
                let rides = land(desk, false, &mut want, &mut trace);
                net.extend(rides.into_iter().map(|r| (k, r)));
                desk.view = h.events.len();
                let v = board(&h, desk.view);
                let rides = match desk.store.ride(J, &g, &v, "drain", Carry::Submit).expect("nothing waits") {
                    Riding::Go(r) => vec![r],
                    Riding::Wait(_) => land(desk, false, &mut want, &mut trace),
                };
                net.extend(rides.into_iter().map(|r| (k, r)));
            }
            for (_, r) in net.drain(..) {
                h.user(&r.text);
            }
            for desk in &mut desks {
                desk.view = h.events.len();
                desk.store.settle(J, &g, &h.vis(), &[]);
            }
            for desk in &mut desks {
                land(desk, false, &mut want, &mut trace);
            }
            if desks.iter().all(|x| x.store.len() == 0 && x.store.waiting_actions() == 0) {
                break;
            }
            assert!(round < 19, "seed {seed}: never settled\n{}", trace.join("\n"));
        }
        let shared = h.vis();
        for n in names {
            let i = id(n);
            for (k, desk) in desks.iter().enumerate() {
                assert_eq!(
                    desk.store.dismissed(J, &g, &i, &shared),
                    shared.dismissed(&i),
                    "seed {seed}, {n}: desk {k}\n{}",
                    trace.join("\n")
                );
                assert_eq!(in_file(&desk.dir).len(), 0, "seed {seed}: desk {k}'s file");
            }
            if owner(n) != b'e' {
                let end = want.get(&i).copied().unwrap_or(false);
                assert_eq!(shared.dismissed(&i), end, "seed {seed}, {n}: the last wish\n{}", trace.join("\n"));
            }
        }
    }
}
