//! A block's revisions, as the walk steps through them (synaps-cards
//! `docs/blocks.md` v1.3 §3.6, §1.8): what the board still holds of them,
//! read from the board drawn ([`Drawn::records`], [`Drawn::tombstones`]),
//! and how a past one is labelled. The logic only: `crate::walk` keeps the
//! view and draws it.
//!
//! - **The order** is newest first: the current incarnation's revisions
//!   (its latest is where the view starts, and is no view), then, behind
//!   `— replaced —`, the revisions of the id's earlier incarnations the
//!   board still holds (each a tombstone's tag, the newest ended first).
//!   A revision record is a reference (§1.8): its body is the re-fold
//!   scratch's, one at a time ([`crate::board`]'s re-folds).
//! - **The label** reads `r3 of 5 · agent · 07:41`: who, and when, from
//!   the row of the event it is at (a history row's message time, a live
//!   row's event time); no time recorded, none shown, never "now". An
//!   earlier incarnation's reads `— replaced — r2 · agent · 07:39`: view
//!   only, never restorable (it was another block).

use cards_spec::block::{Author, Ref, Tag};
use cards_spec::board::Pos;
use synaps_client::blocks::Drawn;

/// One revision the board holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Rev {
    pub tag: Tag,
    pub rev: u32,
    pub at: Pos,
    pub author: Author,
    /// The call of the event it is at: its row's time is found by it.
    pub call: Option<std::sync::Arc<str>>,
    /// Of an earlier incarnation of the id (behind `— replaced —`).
    pub earlier: bool,
}

/// Block `id`'s revisions on board `d`, its current incarnation `tag`'s
/// first, newest first (module docs).
pub(crate) fn revisions(d: &Drawn, id: &Ref, tag: Tag) -> Vec<Rev> {
    let of = |tag: Tag, earlier: bool| {
        let mut v: Vec<Rev> = d
            .records
            .iter()
            .filter(|(r, _)| r.tag == tag)
            .map(|(r, m)| Rev { tag, rev: r.rev, at: r.at, author: r.author, call: m.call.clone(), earlier })
            .collect();
        v.sort_by_key(|r| std::cmp::Reverse(r.rev));
        v
    };
    let mut out = of(tag, false);
    let mut gone: Vec<_> = d.tombstones.iter().filter(|t| &t.id == id && t.tag != tag).collect();
    gone.sort_by_key(|t| std::cmp::Reverse(t.ended));
    for t in gone {
        out.extend(of(t.tag, true));
    }
    out
}

/// Who wrote it.
pub(crate) fn who(a: Author) -> &'static str {
    match a {
        Author::Agent => "agent",
        Author::User => "you",
    }
}

/// A past revision's label (module docs): `latest` is the current
/// incarnation's revision, `time` its row's `HH:MM` if it has one.
pub(crate) fn label(r: &Rev, latest: u32, time: Option<&str>) -> String {
    let mut s = if r.earlier { format!("— replaced — r{}", r.rev) } else { format!("r{} of {latest}", r.rev) };
    s.push_str(" · ");
    s.push_str(who(r.author));
    if let Some(t) = time {
        s.push_str(" · ");
        s.push_str(t);
    }
    s
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cards_spec::board::{Board, Pos};
    use synaps_client::blocks::Drawn;

    use super::*;

    /// The board of the blocks demo, its records and tombstones as a
    /// drawn board carries them.
    fn drawn() -> Drawn {
        let (b, events) = Board::of_history(&synaps_client::synthetic::blocks_history());
        let v = b.view(&events);
        Drawn {
            records: v
                .records()
                .map(|r| (r.clone(), synaps_client::blocks::Mark { row: Some(r.at.event as u64), call: None }))
                .collect::<Vec<_>>()
                .into(),
            tombstones: v.tombstones().cloned().collect::<Vec<_>>().into(),
            log: Arc::from(Vec::new()),
            ..Default::default()
        }
    }

    /// §3.6: the current incarnation's revisions newest first, then the
    /// earlier incarnation's behind `— replaced —`; a block never
    /// replaced has its own only.
    #[test]
    fn a_blocks_revisions_are_newest_first_then_its_earlier_incarnations() {
        let (b, events) = Board::of_history(&synaps_client::synthetic::blocks_history());
        let v = b.view(&events);
        let d = drawn();
        let plan = v.block(&Ref::parse("plan").expect("id")).expect("plan");
        let revs = revisions(&d, &plan.id, plan.tag);
        assert_eq!(revs.iter().map(|r| r.rev).collect::<Vec<_>>(), [3, 2, 1], "plan: put, then two patches");
        assert!(revs.iter().all(|r| !r.earlier && r.tag == plan.tag && r.author == Author::Agent));
        // `now` was replaced: its r1, then the earlier status's r2, r1.
        let now = v.block(&Ref::parse("now").expect("id")).expect("now");
        assert!(now.replaced.is_some());
        let revs = revisions(&d, &now.id, now.tag);
        let seen: Vec<(u32, bool)> = revs.iter().map(|r| (r.rev, r.earlier)).collect();
        assert_eq!(seen, [(1, false), (2, true), (1, true)]);
        assert!(revs[1].tag != now.tag, "the earlier incarnation's own tag");
        assert!(revs[1].at < revs[0].at && revs[2].at < revs[1].at, "older is earlier in the history");
    }

    #[test]
    fn a_revisions_label_reads_who_and_when() {
        let r = Rev {
            tag: Tag::parse("00000000000000000000000000000001").expect("a tag"),
            rev: 3,
            at: Pos::event(4),
            author: Author::Agent,
            call: None,
            earlier: false,
        };
        assert_eq!(label(&r, 5, Some("07:41")), "r3 of 5 · agent · 07:41");
        assert_eq!(label(&r, 5, None), "r3 of 5 · agent", "no time recorded: none, never now");
        let e = Rev { earlier: true, author: Author::User, rev: 2, ..r.clone() };
        assert_eq!(label(&e, 1, Some("07:39")), "— replaced — r2 · you · 07:39");
    }
}
