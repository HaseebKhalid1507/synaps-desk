//! Handing back, in the window (synaps-cards `docs/blocks.md` v1.3 §3.2,
//! §3.4, §3.6, §3.7; §8.2's D8 row, its second half: D8b). The model is
//! [`crate::handback`]; this sends, and draws what it says.
//!
//! - **Hand back** (Enter on a held block, or its button): the pre-check
//!   on the bound board ([`crate::drafts::Draft::handing`]), the memo's
//!   admission ([`crate::handback::HandBacks::may_hand_back`], so the
//!   button and the send never disagree), the notices that ride it, then
//!   the card path's send. The draft then draws over the board marked
//!   `SENDING · r4` (or `QUEUED`, `HELD`, `CHECKING`), takes no key and no
//!   new hold, until its edit is out of flight and the board holds the
//!   history it is in ([`AppView::settle_sent_drafts`]): a drawing only.
//! - **Restore** (`R` on a past revision in the walk, §3.6): an ordinary
//!   edit of the current incarnation, back to the user paths of `rN` that
//!   differ now ([`crate::drafts::Draft::restore`]), line 1 `back to rN:`,
//!   the same pre-check, guards and gate. An earlier incarnation's is
//!   view-only.
//! - **The block's delivery line** ([`AppView::edit_delivery`]): this
//!   desk's latest hand-back of the incarnation while it is out, not
//!   sent, not confirmed or covered by another's, with Retry, Resend
//!   (each behind the same gate, the notices built afresh) and Discard.
//! - **A hand-back's row** ([`AppView::render_hand_back`]): line 1's
//!   words, and what the board says it did (`applied 3 of 4 · deploy
//!   already ran`). Two lines, always: what lands later never changes its
//!   height.
//! - **`changed your edit`** (§3.4 step 6): on a block whose path the
//!   agent overwrote after the user, until the user next focuses it (the
//!   walk's ring on it, or a take). The path, on hover. What was seen is
//!   kept per incarnation and path, at most [`SEEN_MAX`].
//! - **`another edit arrived · r7`** (§5.4): on a held draft, once a
//!   hand-back other than this desk's lands on its incarnation after `B`.

use std::sync::Arc;
use std::time::Instant;

use cards_spec::block::Path;
use cards_spec::block::message::{self, Edit};
use cards_spec::block::{Ref, Tag};
use cards_spec::memo::block as bm;
use cards_spec::memo::{Resend, State};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use synaps_client::blocks::Shown;
use synaps_client::transcript::Item;

use crate::app::AppView;
use crate::desires::{Carry, Ride, Riding};
use crate::fonts::{self, TextPx as _};
use crate::handback::{EditOut, Line1, SENDING};
use crate::live::LiveId;
use crate::theme::{Metrics, Palette};

/// `changed your edit` chips seen, kept at most (§1.8): the oldest goes.
pub(crate) const SEEN_MAX: usize = 256;

/// A restore with nothing to put back.
pub(crate) const NOTHING_TO_RESTORE: &str = "nothing to restore · it is as r{} now";

/// A restore of an earlier incarnation (§3.6): view-only.
pub(crate) const VIEW_ONLY: &str = "an earlier block · view only";

/// What a send is.
#[derive(Debug)]
pub(crate) enum What {
    /// A hand-back, or a restore (boxed: the others are a word).
    New {
        edit: Box<Edit>,
        l1: Line1,
    },
    Retry,
    Resend,
}

/// An edit on its way to the wire (its notices' marks first, maybe).
#[derive(Debug)]
pub(crate) struct EditSend {
    lid: LiveId,
    seq: u64,
    session: String,
    id: Ref,
    tag: Tag,
    body: String,
    what: What,
}

impl AppView {
    /// Enter on the held block the pane edits, or its Hand back: `Err`,
    /// why nothing went.
    pub(crate) fn edit_hand_back(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        let _ = cx;
        Err("stand-in (the tests first) · nothing was sent".into())
    }

    /// `s`, checked, its notices riding (now, or once their marks are
    /// written).
    fn send_edit(&mut self, mut s: EditSend, cx: &mut Context<Self>) -> Result<(), String> {
        let t = &self.model.lives[s.lid].transcript;
        let drawn = t.drawn().cloned();
        let d = drawn.as_deref();
        let memo = &self.model.memos.edits;
        match &s.what {
            What::New { edit, l1 } => {
                memo.may_hand_back(&s.session, edit, l1, t, d)?;
                s.body = message::build(edit, &l1.summary()).map_err(|e| e.to_string())?;
            }
            What::Retry if !memo.may_retry(&s.session, &s.body, t, d) => return Err(crate::lease::CHANGED.into()),
            What::Resend if !memo.may_resend(&s.session, &s.body, t, d) => return Err(crate::lease::CHANGED.into()),
            _ => {}
        }
        let rode = match self.wish_board(s.lid) {
            None => Riding::Go(Ride { text: s.body.clone(), carrier: None, notices: Vec::new() }),
            Some((conv, g, shared)) => self.wishes.desires.ride(&conv, &g, &shared, &s.body, Carry::HandBack).map_err(|r| r.0)?,
        };
        match rode {
            Riding::Go(ride) => self.edit_go(s, ride, cx),
            Riding::Wait(ticket) => {
                if matches!(s.what, What::New { .. }) {
                    self.block_drafts.sent(&s.session, &s.id, s.tag, Some(s.body.clone()));
                }
                self.edit_waits.insert(ticket, s);
                log::info!(
                    target: "synaps_desktop::handback",
                    "a hand-back waits for its notices' marks; {} wait, {} bytes",
                    self.edit_waits.len(),
                    self.edit_waits_bytes()
                );
                self.pump_wishes(cx);
                Ok(())
            }
        }
    }

    /// The notices' marks landed for the edit waiting on `ticket`: it goes
    /// now, `ride` its message. `false`: not an edit's ticket.
    pub(crate) fn edit_landed(&mut self, ticket: u64, ride: Ride, cx: &mut Context<Self>) -> bool {
        let _ = (ticket, ride, cx);
        false
    }

    /// `s` to the wire as `ride` says (its body, the notices then): the
    /// memo takes it, then the card path's send.
    fn edit_go(&mut self, s: EditSend, ride: Ride, cx: &mut Context<Self>) -> Result<(), String> {
        let lid = s.lid;
        let new = matches!(s.what, What::New { .. });
        let text = if self.model.lives[lid].attachment() == Some(s.seq) {
            let trailer = message::split(&ride.text).1;
            let now = Instant::now();
            let m = &mut self.model;
            let t = &m.lives[lid].transcript;
            let drawn = t.drawn().cloned();
            let d = drawn.as_deref();
            match &s.what {
                What::New { edit, l1 } => m.memos.edits.hand_back(&s.session, edit, l1, &trailer, t, d, now),
                What::Retry => m.memos.edits.retry(&s.session, &s.body, &trailer, t, d, now).ok_or_else(|| crate::lease::CHANGED.into()),
                What::Resend => match m.memos.edits.resend(&s.session, &s.body, &trailer, t, d, now) {
                    Resend::Send(text) => Ok(text),
                    Resend::Checking => Err(crate::deliver::UNSETTLED.into()),
                    Resend::No => Err(crate::lease::CHANGED.into()),
                },
            }
        } else {
            Err("the session changed · nothing was sent".into())
        };
        let text = match text {
            Ok(t) => t,
            Err(why) => {
                self.ride_went(lid, &ride.text, ride.carrier, false);
                if new {
                    self.block_drafts.sent(&s.session, &s.id, s.tag, None);
                }
                cx.notify();
                return Err(why);
            }
        };
        let busy = self.model.lives[lid].transcript.busy();
        let out = EditOut { seq: s.seq, busy, text, body: s.body.clone() };
        #[cfg(test)]
        self.edit_sends.push(out.clone());
        let sent = match &self.client {
            Some(c) if busy => c.on(out.seq).steer(out.text.clone()).is_ok(),
            Some(c) => c.on(out.seq).submit(out.text.clone()).is_ok(),
            None => false,
        };
        if !sent {
            const STOPPED: &str = "not sent · the connection thread has stopped";
            self.ride_went(lid, &out.text, ride.carrier, false);
            if new {
                self.model.memos.edits.forget(&s.session, &s.body);
                self.block_drafts.sent(&s.session, &s.id, s.tag, None);
            } else {
                self.model.card_unsent(lid, STOPPED);
            }
            cx.notify();
            return Err(STOPPED.into());
        }
        log::info!(
            target: "synaps_desktop::handback",
            "hand-back of {}: {} ({} bytes); the desk keeps {} hand-back memos, {} bytes",
            s.id,
            if busy { "steer" } else { "submit" },
            out.text.len(),
            self.model.memos.edits.len(),
            self.model.memos.edits.bytes()
        );
        self.model.lives[lid].edit_sent(out.text.clone());
        self.ride_went(lid, &out.text, ride.carrier, true);
        if new {
            self.block_drafts.sent(&s.session, &s.id, s.tag, Some(s.body));
        }
        self.after_card(crate::blocks::row_id(&s.id), cx);
        Ok(())
    }

    /// Retry, Resend or Discard on block `id`'s incarnation `tag` in the
    /// pane acted on: this desk's latest hand-back of it.
    pub(crate) fn edit_again(&mut self, id: &Ref, tag: Tag, what: Option<What>, cx: &mut Context<Self>) {
        let _ = (id, tag, what, cx);
    }

    /// `R` on the walked block's past revision (§3.6): a restore of the
    /// current incarnation to it, sent as an edit. `Err`: why not.
    pub(crate) fn restore(&mut self, row: u64, cx: &mut Context<Self>) -> Result<(), String> {
        let _ = (row, cx);
        Err("stand-in (the tests first) · nothing was sent".into())
    }

    /// Each session's handed-back drafts that are done with go (module
    /// docs): their edit out of flight, and the board holding the history
    /// it is in (an outcome on its row; or it never got there).
    pub(crate) fn settle_sent_drafts(&mut self) {}

    /// Block row `row` measured again in every pane that draws it.
    pub(crate) fn remeasure_block(&mut self, row: u64) {
        for pane in self.panes.iter_mut() {
            if let Some(ix) = pane.disp.iter().position(|d| d.id == row && matches!(d.kind, crate::turns::Kind::Block { .. })) {
                pane.list.remeasure_items(ix..ix + 1);
            }
        }
    }

    /// The draft's header while it is sent (module docs): its delivery
    /// word.
    pub(crate) fn sent_head(&self, d: &crate::drafts::Draft) -> Option<String> {
        let _ = d;
        None
    }

    /// The words under a sent draft: its delivery's, as the memo says.
    pub(crate) fn sent_words(&self, d: &crate::drafts::Draft) -> Option<String> {
        let _ = d;
        None
    }

    /// Whether the memo admits the hand-back of the draft the pane edits
    /// now (the button's `on`), or why not.
    pub(crate) fn may_hand_back(&self, d: &crate::drafts::Draft) -> Result<(), String> {
        let _ = d;
        Err("stand-in (the tests first) · nothing was sent".into())
    }

    /// Block `s`'s delivery line (row `id`): this desk's latest hand-back of
    /// its incarnation, while it says something (module docs).
    pub(crate) fn edit_delivery(&self, s: &Shown, id: u64, p: &Palette, m: &Metrics, cx: &mut Context<Self>) -> Option<AnyElement> {
        let _ = (s, id, p, m, cx);
        None
    }

    /// A hand-back's row (module docs): line 1's words, then what the
    /// board says it did. `item` is its user message.
    pub(crate) fn render_hand_back(&self, item: &Item, id: u64, p: &Palette, m: &Metrics) -> AnyElement {
        let body = message::split(&item.text).0;
        let line1 = body.split('\n').next().unwrap_or_default();
        let (head, words) = match line1.split_once("] ") {
            Some((h, w)) => (h.trim_start_matches("[block "), w),
            None => ("", line1),
        };
        let words = crate::ask::shown(words, cards_spec::Lines::Single);
        let edit = message::parse(body);
        let l = &self.model.lives[self.panes.cur.live];
        let drawn = l.transcript.drawn();
        let outcome = drawn.and_then(|d| d.outcomes.iter().find(|o| o.row == Some(item.id)).map(|o| o.outcome.clone()));
        // Its incarnation isn't this conversation generation's (§5.2): the
        // queued message moved to the successor, where its id names no
        // block (and there may be no board at all).
        let compacted = l.transcript.compactions() > 0
            && edit.as_ref().is_some_and(|e| {
                drawn.is_none_or(|d| {
                    !d.records.iter().any(|(r, _)| r.tag == e.of)
                        && !d.tombstones.iter().any(|t| t.tag == e.of)
                        && d.board.as_deref().is_none_or(|st| st.block(&e.id).is_none_or(|b| b.tag != e.of))
                })
            });
        let said = match outcome {
            Some(o) => Some(crate::handback::outcome_words(&o, edit.as_ref(), compacted)),
            None if compacted => Some(crate::handback::COMPACTED_AWAY.into()),
            None => None,
        };
        let (h, w, s) = (head.to_string(), words.to_string(), said.clone().unwrap_or_default());
        v_flex()
            .relative()
            .w_full()
            .bg(p.user)
            .px(px(m.pad_x))
            .py(px(m.pad_y))
            .font_family(fonts::SANS)
            .text_px(m.body)
            .line_height(px(m.body_line))
            .whitespace_nowrap()
            .debug_selector(move || format!("hand-back {id} {h} · {w} · {s}"))
            .child(
                h_flex()
                    .w_full()
                    .gap(px(8.))
                    .child(div().flex_none().text_color(p.dim_u).child(crate::glyph::draw(crate::glyph::Glyph::Elbow, p.dim_u)))
                    .child(div().flex_none().font_family(fonts::MONO).text_px(m.meta).text_color(p.dim_u).child(head.to_string()))
                    .child(div().min_w_0().flex_1().overflow_hidden().text_ellipsis().text_color(p.ink).child(words)),
            )
            .child(
                div()
                    .w_full()
                    .h(px(m.body_line))
                    .overflow_hidden()
                    .text_ellipsis()
                    .font_family(fonts::MONO)
                    .text_px(m.meta)
                    .text_color(p.dim_u)
                    .debug_selector(move || format!("hand-back-outcome {id}"))
                    .children(said),
            )
            .child(crate::glyph::bar_at(p.you_mark, 1.))
            .into_any_element()
    }

    /// The `changed your edit` chip of block `s`, if it shows (module
    /// docs): its path, for the hover.
    pub(crate) fn changed_your_edit(&self, s: &Shown) -> Option<Path> {
        let _ = s;
        None
    }

    /// The user focused block row `row` (the walk's ring on it, or a
    /// take): its `changed your edit`, seen.
    pub(crate) fn saw_changed(&mut self, row: u64) {
        let _ = row;
    }

    /// What the `changed your edit` chips seen keep (bytes).
    /// What the hand-backs waiting for their notices' marks hold: each
    /// one's body (a copy of its draft's or its entry's, ≤ B4's ceiling).
    pub(crate) fn edit_waits_bytes(&self) -> usize {
        self.edit_waits.capacity() * (std::mem::size_of::<u64>() + std::mem::size_of::<EditSend>())
            + self.edit_waits.values().map(|s| s.session.capacity() + s.body.capacity()).sum::<usize>()
    }

    pub(crate) fn changed_seen_bytes(&self) -> usize {
        self.changed_seen.capacity() * std::mem::size_of::<(String, Ref, Tag, Path)>()
            + self.changed_seen.iter().map(|(s, _, _, p)| s.capacity() + p.as_str().len()).sum::<usize>()
    }
}
