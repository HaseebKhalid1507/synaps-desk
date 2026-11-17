//! The authorize card in the log (polish.md §2.6, F3, F6, F7): a prompt's
//! own list row, directly after the call it gates (`waiting_on`: the last
//! unfinished call of the turn), or at the turn's end for a secret or a
//! confirm with no unfinished call. Flat on `card`, its 1 px `warn_mark`
//! state frame (traced on arrival, motion.md §6.3) and 2 px dashes along its
//! top; a header with what asked and when; the well; the folder; Allow all
//! (input owner only); DENY / ALLOW.
//!
//! Its state is one long-lived [`PromptCard`] per pending prompt, owned by
//! the [`AppView`] and only drawn by its row: the list builds rows while
//! they are visible, and the card's arm, read state, well scroll and the
//! secret field's text outlive that.
//!
//! Answered, the row collapses on `sheet` (P13, through
//! [`crate::motion::grow_block`]), its content at full strength until
//! clipped away, and the call's row says what happened in its meta:
//! `allowed 14:21 · 7s`, `denied 14:22`, `answered elsewhere 14:21`. A
//! prompt with no call row (a secret, a confirm with nothing unfinished)
//! keeps a one-line row: `secret · provided 14:21`, never the value.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::component::input::RopeExt as _;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use synaps_client::protocol::PromptRequest;
use synaps_client::transcript::Item;

use crate::app::{ARM, AppView, Arm, Fill, YES_KEY, authorize_ready, display, fbtn_charge, fbtn_key, hidden, lines_below, mono, tbtn};
use crate::fonts::TextPx as _;
use crate::fonts::{self, label};
use crate::glyph;
use crate::model::{CONFIRM_NO, Model};
use crate::theme::{Metrics, Palette};
use crate::turns::{Disp, Kind};

/// A prompt row's id: its own count with this bit (item ids count up from 1;
/// `DAY_ID` is `1 << 63`).
pub(crate) const PROMPT_ID: u64 = 1 << 61;

/// The key that allows, as the button shows it.
pub(crate) const ALLOW_KEY: &str = if cfg!(target_os = "macos") { "⌘ ↵" } else { "ctrl ↵" };

/// The dashes along the card's top edge (F7): 8 on, 4 off, 2 px.
const DASH: f32 = 8.;
const DASH_GAP: f32 = 4.;
const DASH_H: f32 = 2.;
/// The allow-all box (drawn: 1 px frame, a 6×6 square when checked).
const CHECK: f32 = 12.;
const CHECK_IN: f32 = 6.;

/// Receipts kept per session (the oldest go first).
pub(crate) const RECEIPTS: usize = 64;

/// Now, local `HH:MM` (a receipt's time).
pub(crate) fn hhmm_now() -> SharedString {
    chrono::Local::now().format("%H:%M").to_string().into()
}

/// What the card's last live frame showed of its state: the collapse
/// draws the same, so nothing but its clip changes while it goes.
#[derive(Clone, Default)]
pub(crate) struct CardLook {
    pub(crate) can: bool,
    pub(crate) ready: bool,
    pub(crate) keys: bool,
    pub(crate) in_field: bool,
    pub(crate) typed: bool,
    pub(crate) footer: Option<SharedString>,
    pub(crate) footer_warn: bool,
}

/// The prompt the card shows, kept while it is pending.
pub(crate) struct PromptCard {
    /// [`Model::card_key`]: `(epoch, prompt id)`.
    pub(crate) key: (u64, u64),
    /// Its list row's id ([`PROMPT_ID`] and a count: never reused).
    pub(crate) row: u64,
    /// A secret prompt's.
    pub(crate) secret: bool,
    pub(crate) arm: Arm,
    /// The prompt as it came (its title, its kind, its scope): what the
    /// card says while it collapses after the model has dropped it.
    pub(crate) pr: PromptRequest,
    /// The body, verbatim (blank lines kept).
    pub(crate) body: SharedString,
    /// What the top bar and the stub say about it ([`crate::chrome::prompt_gist`]).
    pub(crate) gist: SharedString,
    /// A guard's shape: the command alone in the well, the folder under it.
    pub(crate) guard: Option<Guard>,
    /// `bash · guard · asked 14:21` ([`asked`]).
    pub(crate) asked: Option<SharedString>,
    pub(crate) scroll: ScrollHandle,
    /// The well's bar: each card its own, so a drag on one never carries
    /// into the next.
    pub(crate) bar: crate::scrollbar::BarState,
    /// The buttons' line: where it was laid out ([`PromptCard::buttons_shown`]).
    pub(crate) actions: ScrollHandle,
    /// The well was scrolled to its end (or never needed to be), on screen.
    pub(crate) read: bool,
    /// Allow all is checked: ALLOW sends `"always"`.
    pub(crate) allow_all: bool,
    /// Its row's height as last drawn (what a collapse starts from).
    pub(crate) height: Rc<Cell<f32>>,
    /// The item its row follows (an id), and whether that is its own
    /// line once answered (no call row to carry the receipt).
    pub(crate) after: Cell<Option<(u64, bool)>>,
    /// Repaints once the arm delay is over.
    pub(crate) _arm: Option<Task<()>>,
}

impl PromptCard {
    /// Keep `card` with the prompt on top of `m` ([`Model::card_key`]): the
    /// same key keeps its arm and read state; a new prompt, the same id in
    /// another attachment or after a reconnect (a restarted daemon numbers
    /// its prompts from 1 again) starts over, unarmed and unread, at `now`.
    /// True when the card is new. `view`: the bottom of what is on screen
    /// of the list (the window's height before it is laid out).
    pub(crate) fn sync(card: &mut Option<PromptCard>, m: &Model, now: Instant, view: f32) -> bool {
        match (m.card_key(), card.as_mut()) {
            (Some(k), Some(c)) if c.key == k => {
                if c.below(view).is_some_and(|b| b <= 1.) {
                    c.read = true;
                }
                false
            }
            (Some(k), _) => {
                let pr = &m.transcript.pending_prompts()[0];
                *card = Some(PromptCard::new(k, pr, now));
                true
            }
            (None, _) => {
                *card = None;
                false
            }
        }
    }

    fn new(key: (u64, u64), pr: &PromptRequest, now: Instant) -> Self {
        static ROWS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let secret = pr.is_secret();
        PromptCard {
            key,
            row: PROMPT_ID | ROWS.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            secret,
            arm: Arm::new(now),
            body: crate::app::well_body(&pr.prompt),
            gist: crate::chrome::prompt_gist(&pr.title, &pr.prompt).into(),
            guard: (!secret).then(|| guard_shape(&pr.prompt)).flatten(),
            asked: asked(pr).map(Into::into),
            pr: pr.clone(),
            scroll: ScrollHandle::new(),
            bar: Default::default(),
            actions: ScrollHandle::new(),
            read: false,
            allow_all: false,
            height: Rc::new(Cell::new(f32::NAN)),
            after: Cell::new(None),
            _arm: None,
        }
    }

    pub(crate) fn armed(&self) -> bool {
        self.arm.armed(Instant::now())
    }

    pub(crate) fn keys_armed(&self) -> bool {
        self.arm.keys_armed(Instant::now())
    }

    /// What of the well is out of sight with the list's view ending at
    /// `view`, once it has been laid out: below its own view, or under the
    /// list's bottom (the dock and the composer are not a place to read).
    pub(crate) fn below(&self, view: f32) -> Option<f32> {
        let b = self.scroll.bounds();
        if b.size.height <= px(0.) {
            return None;
        }
        Some(hidden(f32::from(self.scroll.max_offset().y + self.scroll.offset().y), f32::from(b.bottom()), view))
    }

    /// Its buttons were laid out whole inside the list's view `top..view`
    /// (or not laid out yet: the frame that draws a new card).
    pub(crate) fn buttons_shown(&self, top: f32, view: f32) -> bool {
        let b = self.actions.bounds();
        b.size.height <= px(0.) || (f32::from(b.bottom()) <= view + 0.5 && f32::from(b.top()) >= top - 0.5)
    }
}

/// The answered card collapsing in its row (P13): the card as its last
/// frame drew it, with no listener (a collapse takes no click), clipped
/// from its height to `to` on `sheet`.
pub(crate) struct CardExit {
    pub(crate) card: PromptCard,
    pub(crate) look: CardLook,
    pub(crate) at: Instant,
    /// The session it was in (its row is that list's).
    pub(crate) session: String,
    /// The item its row follows.
    pub(crate) after: u64,
    /// Where its height lands: 0, or a receipt line's.
    pub(crate) to: f32,
    pub(crate) sink: Rc<Cell<f32>>,
}

impl CardExit {
    /// Its shown height at `now` (`None`: not measured), and whether the
    /// collapse has landed.
    pub(crate) fn shown(&self, now: Instant) -> (Option<f32>, bool) {
        let from = self.card.height.get();
        if from.is_nan() {
            return (None, true);
        }
        let t = now.saturating_duration_since(self.at).as_secs_f32();
        let x = crate::motion::Spring::from_rest(1., crate::motion::SHEET_RESPONSE, t);
        let h = self.to + (from - self.to).max(0.) * x;
        (Some(h), (h - self.to).abs() < 0.5)
    }
}

/// What became of a prompt (polish.md §2.6, the receipt).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Answer {
    Allowed,
    AllowedAll,
    Denied,
    Provided,
    Cancelled,
    Elsewhere,
}

/// An answered prompt's receipt, kept for its session while the window
/// lives (the daemon keeps no record of who answered when).
#[derive(Clone, Debug)]
pub(crate) struct Receipt {
    /// The call's row it sits in, or (`own`) the item its own row follows.
    pub(crate) after: u64,
    pub(crate) own: bool,
    pub(crate) row: u64,
    pub(crate) secret: bool,
    pub(crate) answer: Answer,
    /// Local `HH:MM` of the answer.
    pub(crate) at: SharedString,
    /// How long the card waited for it here.
    pub(crate) took: Option<Duration>,
    /// When it landed: the receipt lifts in over `QUICK`.
    pub(crate) landed: Instant,
}

impl Receipt {
    /// `allowed 14:21 · 7s`, `denied 14:22`, `answered elsewhere 14:21`;
    /// a secret's `provided 14:21`. The first word and the rest.
    pub(crate) fn words(&self) -> (&'static str, String) {
        let word = match self.answer {
            Answer::Allowed => "allowed",
            Answer::AllowedAll => "allowed all",
            Answer::Denied => "denied",
            Answer::Provided => "provided",
            Answer::Cancelled => "cancelled",
            Answer::Elsewhere => "answered elsewhere",
        };
        let took = self.took.filter(|_| self.answer != Answer::Elsewhere).map(|d| format!(" · {}", took(d))).unwrap_or_default();
        (word, format!(" {}{took}", self.at))
    }
}

/// `7s`, `2m 05s`, `1h 03m`.
pub(crate) fn took(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m {:02}s", s / 60, s % 60),
        _ => format!("{}h {:02}m", s / 3600, s / 60 % 60),
    }
}

/// An answer this window sent, until its prompt resolves (or the daemon
/// refuses it: then it is forgotten and the card asks again).
#[derive(Clone, Debug)]
pub(crate) struct Sent {
    pub(crate) key: (u64, u64),
    pub(crate) answer: Answer,
    pub(crate) at: SharedString,
    pub(crate) took: Duration,
}

/// A guard's confirm (polish.md §2.6, the well): a `$ ` command line after
/// a line that introduces it with `… in <folder>:`, read as
/// [`crate::chrome::prompt_gist`] reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Guard {
    pub(crate) command: SharedString,
    pub(crate) dir: SharedString,
}

pub(crate) fn guard_shape(body: &str) -> Option<Guard> {
    let lines: Vec<&str> = body.lines().map(str::trim).collect();
    let first_after = |at: usize| (at + 1..lines.len()).find(|&i| !lines[i].is_empty());
    let intro = lines.iter().position(|l| l.starts_with("Message from ") && l.ends_with(':')).and_then(first_after)?;
    let command = lines.get(first_after(intro)?)?.strip_prefix("$ ")?;
    let line = lines[intro].strip_suffix(':')?;
    let dir = line.rsplit_once(" in ")?.1.trim();
    (!dir.is_empty() && !command.trim().is_empty()).then(|| Guard { command: command.to_string().into(), dir: dir.to_string().into() })
}

/// The header's right: `bash · guard · asked 14:21`, from the scope's tool
/// and extensions and `raised_at` (local, 24 h); a missing part goes with
/// its separator.
pub(crate) fn asked(pr: &PromptRequest) -> Option<String> {
    let scope = pr.confirm_scope.as_ref();
    let parts: Vec<String> = [
        scope.and_then(|s| s.tool.clone()).filter(|t| !t.is_empty()),
        scope.map(|s| s.extensions_label()).filter(|e| !e.is_empty()),
        pr.raised_at.as_deref().and_then(crate::times::Stamp::local).map(|t| format!("asked {}", t.hhmm)),
    ]
    .into_iter()
    .flatten()
    .collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// A prompt's row in the list: after item `after` (an index), its id and
/// rev (a receipt line's differs from the card's).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct PromptRow {
    pub(crate) after: usize,
    pub(crate) id: u64,
    pub(crate) rev: u64,
}

/// `out` (the rows of `items[from..]`) with the prompt rows that follow
/// those items, each after the last row of its item (after a folded run's
/// summary when its item is in the run).
pub(crate) fn with_prompts(mut out: Vec<Disp>, prompts: &[PromptRow], from: usize) -> Vec<Disp> {
    for r in prompts.iter().rev().filter(|r| r.after >= from) {
        let at = out.partition_point(|d| d.item <= r.after);
        out.insert(
            at,
            Disp { item: r.after, id: r.id, rev: r.rev, kind: Kind::Prompt, gap: crate::turns::Gap::None, who: crate::turns::Who::None },
        );
    }
    out
}

/// The prompt rows in `disp`, as built.
pub(crate) fn built(disp: &[Disp]) -> Vec<PromptRow> {
    disp.iter().filter(|d| d.kind == Kind::Prompt).map(|d| PromptRow { after: d.item, id: d.id, rev: d.rev }).collect()
}

/// The index of item `id` in `items` (ids count up).
fn index_of(items: &[Item], id: u64) -> Option<usize> {
    let i = items.partition_point(|x| x.id < id);
    items.get(i).filter(|x| x.id == id).map(|_| i)
}

impl AppView {
    /// The prompt rows the list should hold now: the live card's (after the
    /// call it waits on, else at the end), the collapsing card's, and the
    /// receipt lines of this session's own-row prompts. None over a
    /// worker's view.
    pub(crate) fn prompt_rows(&self) -> Vec<PromptRow> {
        let mut out = Vec::new();
        if self.model.watch.focus().is_some() {
            return out;
        }
        let Some(sid) = self.model.active.as_ref().map(|a| a.session_id.as_str()) else { return out };
        let items = self.model.shown().items();
        if items.is_empty() {
            return out;
        }
        // The collapsing card's row becomes its receipt line when it lands.
        let exit = self.card_exit.as_ref().filter(|e| e.session == sid);
        for r in self.receipts.get(sid).into_iter().flatten().filter(|r| r.own && exit.is_none_or(|e| e.card.row != r.row)) {
            if let Some(after) = index_of(items, r.after) {
                out.push(PromptRow { after, id: r.row, rev: 1 });
            }
        }
        if let Some(e) = exit
            && let Some(after) = index_of(items, e.after)
        {
            out.push(PromptRow { after, id: e.card.row, rev: 0 });
        }
        if let Some(c) = self.current_card() {
            // The call it gates, or the last item.
            let call = self.waiting_on.filter(|_| !c.secret).and_then(|id| index_of(items, id));
            let after = call.unwrap_or(items.len() - 1);
            c.after.set(Some((items[after].id, call.is_none())));
            out.push(PromptRow { after, id: c.row, rev: 0 });
        }
        out.sort();
        out
    }

    /// The card's row in the list, if built.
    pub(crate) fn card_ix(&self) -> Option<usize> {
        let row = self.current_card()?.row;
        self.disp.iter().rposition(|d| d.kind == Kind::Prompt && d.id == row)
    }

    /// The list's view, top and bottom in the window (the window when it
    /// has not been laid out).
    /// Read from the last layout as a frame begins ([`AppView::read_list_view`]):
    /// a row drawing inside the list's layout may not ask the list.
    pub(crate) fn list_view(&self) -> (f32, f32) {
        self.list_box.get()
    }

    /// The list's view as its last layout left it (as a frame begins).
    pub(crate) fn read_list_view(&self) {
        let b = self.list.viewport_bounds();
        let v = if b.size.height <= px(0.) { (0., self.height) } else { (f32::from(b.top()), f32::from(b.bottom())) };
        self.list_box.set(v);
    }

    /// The card can be answered from the screen: its row is in the list's
    /// view with its buttons whole (as the frame began: [`AppView::read_card_on_screen`]).
    pub(crate) fn card_on_screen(&self) -> bool {
        self.card_seen.get()
    }

    /// [`AppView::card_on_screen`] from the list's last layout, as a frame
    /// begins (a row drawing inside the list's layout may not ask the list).
    pub(crate) fn read_card_on_screen(&self) {
        self.card_seen.set(self.card_on_screen_now());
    }

    fn card_on_screen_now(&self) -> bool {
        let Some(c) = self.current_card() else { return false };
        let (top, bottom) = self.list_view();
        let Some(ix) = self.card_ix() else { return false };
        match self.list.bounds_for_item(ix) {
            Some(b) => f32::from(b.top()) < bottom && f32::from(b.bottom()) > top && c.buttons_shown(top, bottom),
            // Not drawn yet: the frame that draws a new card (at the end,
            // where the list follows) counts it shown, as its buttons do.
            None => c.height.get().is_nan() && self.list.is_following_tail(),
        }
    }

    /// Ctrl+J: the card's row into view (on F1's glide when it was drawn,
    /// at once when not), from a worker's view back to the session first,
    /// and the keys to it (a secret's to its field).
    pub(crate) fn jump_to_card(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.model.transcript.pending_prompts().is_empty() {
            return;
        }
        if self.model.tabs.view().is_some() {
            return;
        }
        if self.model.watch.focus().is_some() {
            self.open_parent_card(cx);
            self.jump_card = true;
            cx.notify();
            return;
        }
        self.reveal_card(window, cx);
    }

    /// The card's row in view and the keys to it (after a frame has built
    /// it, [`AppView::jump_card`]).
    pub(crate) fn reveal_card(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.card_ix() else { return };
        let (top, bottom) = self.list_view();
        match self.list.bounds_for_item(ix) {
            Some(b) => {
                let (t, bt) = (f32::from(b.top()), f32::from(b.bottom()));
                // Its bottom to the view's (its top if it is taller).
                let by = if t < top || bt - t > bottom - top {
                    t - top
                } else if bt > bottom {
                    bt - bottom
                } else {
                    0.
                };
                if by.abs() > 0.5 {
                    self.list.pause_following_tail();
                    let cut = self.scroller.jump(by, false, bottom - top, self.fx.clock());
                    if cut != 0. {
                        self.list.scroll_by(px(cut));
                    }
                }
            }
            None => self.list.scroll_to_reveal_item(ix),
        }
        let secret = self.current_card().is_some_and(|c| c.secret);
        if secret {
            self.secret_input.update(cx, |s, cx| s.focus(window, cx));
        } else {
            window.focus(&self.focus, cx);
        }
        cx.notify();
    }

    /// Space with the card's keys: Allow all on or off (when offered).
    pub(crate) fn toggle_allow_all(&mut self, cx: &mut Context<Self>) {
        let offered = self.allow_all_offered();
        if let Some(c) = self.card.as_mut().filter(|_| offered) {
            c.allow_all = !c.allow_all;
            log::info!(target: "synaps_desktop::card", "card {:?}: allow all {}", c.key, if c.allow_all { "on" } else { "off" });
            cx.notify();
        }
    }

    /// Allow all is offered on the card: its prompt has a scope to latch,
    /// and this client owns the session's input (the daemon refuses
    /// `"always"` from anyone else: `actor.rs confirm_answer_refusal`).
    pub(crate) fn allow_all_offered(&self) -> bool {
        let owner = self.model.active.as_ref().is_some_and(|a| a.owner == Some(a.client));
        owner && self.current_card().is_some_and(|c| c.pr.allow_all().is_some())
    }

    /// A tool row's receipt (its meta's last part), lifting in over `QUICK`.
    pub(crate) fn receipt_el(&self, item: u64, p: &Palette) -> Option<AnyElement> {
        if self.model.watch.focus().is_some() {
            return None;
        }
        let sid = self.model.active.as_ref()?.session_id.as_str();
        let r = self.receipts.get(sid)?.iter().rev().find(|r| !r.own && r.after == item)?;
        Some(self.receipt_words(r, p).into_any_element())
    }

    fn receipt_words(&self, r: &Receipt, p: &Palette) -> Div {
        let (word, rest) = r.words();
        let k = self.fx.shot(Some(r.landed), Duration::ZERO, crate::motion::QUICK, &crate::motion::ease::OUT, crate::motion::Kind::Fade);
        let lift = crate::motion::LIFT + (1. - crate::motion::LIFT) * k;
        let word_color = if r.answer == Answer::Denied { p.err_o } else { p.dim_t };
        let row = r.row;
        crate::motion::lift(
            h_flex()
                .debug_selector(move || format!("receipt {row}"))
                .flex_none()
                .whitespace_nowrap()
                .child(div().text_color(word_color).child(word))
                .child(div().text_color(p.dim_t).child(SharedString::from(rest))),
            lift,
        )
    }

    /// A prompt row ([`Kind::Prompt`]): the live card, the answered card
    /// collapsing, or an own-row receipt line.
    pub(crate) fn render_authorize(&self, d: Disp, p: &Palette, m: &Metrics, cx: &mut Context<Self>) -> AnyElement {
        let r = m.r;
        if let Some(c) = self.current_card().filter(|c| c.row == d.id) {
            let height = c.height.clone();
            return div()
                .w_full()
                .child(card_slot(self.render_card(c, None, p, m, cx), m))
                .on_children_prepainted(move |b, _, _| {
                    if let Some(b) = b.first() {
                        height.set(f32::from(b.size.height));
                    }
                })
                .into_any_element();
        }
        if let Some(e) = self.card_exit.as_ref().filter(|e| e.card.row == d.id) {
            let body = card_slot(self.render_card(&e.card, Some(&e.look), p, m, cx), m).into_any_element();
            let (shown, _) = e.shown(self.fx.now());
            self.fx.want_frame();
            let g = crate::motion::Grow { key: d.id, shown, sink: e.sink.clone(), travel: 0., strength: 1. };
            return crate::motion::grow_block(body, None, None, g);
        }
        let sid = self.model.active.as_ref().map(|a| a.session_id.as_str()).unwrap_or_default();
        let Some(rc) = self.receipts.get(sid).and_then(|v| v.iter().find(|x| x.row == d.id)) else { return div().into_any_element() };
        let kind = if rc.secret { "secret" } else { "confirm" };
        h_flex()
            .w_full()
            .h(px(receipt_line(m)))
            .items_center()
            .text_px(m.meta)
            .font_family(fonts::MONO)
            .whitespace_nowrap()
            .debug_selector(move || format!("receipt-row {}", d.id))
            .child(div().pl(px((16. * r).round())).text_color(p.dim_t).child(format!("{kind} · ")))
            .child(self.receipt_words(rc, p))
            .into_any_element()
    }

    /// The card. `look`: the collapsing card's last live frame (no
    /// listener, its state as it was); `None`: the live card.
    fn render_card(&self, c: &PromptCard, look: Option<&CardLook>, p: &Palette, m: &Metrics, cx: &mut Context<Self>) -> Stateful<Div> {
        let r = m.r;
        let pad = (16. * r).round();
        let live = look.is_none();
        let pr = &c.pr;
        let secret = c.secret;
        let sent = live && self.model.prompt_answered(pr.id);
        let armed = live && c.armed();
        let keys_state = if live { self.card_keys.get() } else { None };
        let can = look.map_or_else(|| self.drives() && self.model.may_answer(pr.id) && armed, |l| l.can);
        let (_, view) = self.list_view();
        let below = c.below(view);
        let ready = look.map_or_else(|| authorize_ready(c.armed(), below, c.read), |l| l.ready);
        let detached = self.model.active.is_none();
        let in_field = look.map_or(keys_state == Some(true), |l| l.in_field);
        let keys = look.map_or_else(|| keys_state.is_some() && can && c.keys_armed(), |l| l.keys);
        let key_hint = |k: &'static str| mono(k, m.meta, p.dim_k).flex_none();
        let typed = look.map_or_else(|| secret && !self.secret_input.read(cx).value().is_empty(), |l| l.typed);
        let key = live.then_some(c.key);
        let offered = live && self.allow_all_offered();
        let allow_all = offered && c.allow_all;
        let actions =
            if detached {
                None
            } else if secret {
                Some(
                    h_flex()
                        .gap(px(24.))
                        .items_center()
                        .child(
                            h_flex()
                                .gap(px(8.))
                                .items_center()
                                .child(
                                    div().w(px((260. * r).round())).child(
                                        gpui_kit::component::input::Input::new(&self.secret_input)
                                            .disabled(!can)
                                            .text_px(m.mono)
                                            .border_color(p.frame),
                                    ),
                                )
                                // Ctrl/Cmd+Enter takes the keys into the field.
                                .when(keys && !in_field, |d| d.child(key_hint(YES_KEY))),
                        )
                        .child(
                            h_flex()
                                .gap(px(8.))
                                .items_center()
                                .child(
                                    tbtn("p-cancel", label::CANCEL, if can { p.err_k } else { p.dim_k }, if can { p.ink } else { p.dim_k })
                                        .text_px(m.label)
                                        .when(live && can, |d| {
                                            d.on_click(cx.listener(|this, _, window, cx| this.answer_secret(false, window, cx)))
                                        }),
                                )
                                .when(keys, |d| d.child(key_hint("esc"))),
                        )
                        .child(
                            fbtn_key(
                                "p-submit",
                                label::SUBMIT,
                                (in_field && can && typed).then_some(if cfg!(target_os = "macos") { "↵" } else { "enter" }),
                                can && typed,
                                Fill::Warn,
                                p,
                                m,
                            )
                            .when(live && can && typed, |d| {
                                d.on_click(cx.listener(|this, _, window, cx| this.answer_secret(true, window, cx)))
                            }),
                        ),
                )
            } else {
                let yes = can && ready;
                // ALLOW charges (motion.md §6.3): the arm's wait drawn as 10
                // cells lit left to right, 50 ms each; the last lit is the arm.
                // A held read gate keeps them dark.
                let charge = Some(c)
                    .filter(|c| {
                        let may = self.drives() && self.model.may_answer(pr.id);
                        live && may && !yes && self.fx.level() == crate::motion::Level::Full && !(below.is_some_and(|b| b > 1.) && !c.read)
                    })
                    .map(|c| {
                        let cell = ARM / 10;
                        let n = (self.fx.now().saturating_duration_since(c.arm.shown_at()).as_nanos() / cell.as_nanos()).min(10) as u32;
                        if n < 10 {
                            self.fx.want_at(c.arm.shown_at() + cell * (n + 1));
                        }
                        n
                    });
                Some(
                    h_flex()
                        .gap(px(24.))
                        .items_center()
                        .child(
                            h_flex()
                                .gap(px(8.))
                                .items_center()
                                .child(
                                    tbtn("p-no", label::DENY, if can { p.err_k } else { p.dim_k }, if can { p.ink } else { p.dim_k })
                                        .text_px(m.label)
                                        .when_some(key.filter(|_| can), |d, key| {
                                            d.on_click(cx.listener(move |this, _, _, cx| {
                                                this.answer(key, Some(CONFIRM_NO.into()));
                                                cx.notify();
                                            }))
                                        }),
                                )
                                .when(keys, |d| d.child(key_hint("esc"))),
                        )
                        // Clickable whenever the card may be answered: the read
                        // gate is checked at the click (as the keys do), not by
                        // this frame, laid out with the last one's bounds.
                        .child(
                            fbtn_charge("p-yes", label::ALLOW, (keys && yes).then_some(ALLOW_KEY), yes, charge, Fill::Warn, p, m)
                                .when_some(key.filter(|_| can), |d, key| {
                                    d.on_click(cx.listener(move |this, _, _, cx| {
                                        this.authorize(key);
                                        cx.notify();
                                    }))
                                }),
                        ),
                )
            };
        let more = below.filter(|&b| b > 1.).map(|b| lines_below(b, m.code_line)).filter(|_| !c.read);
        let prompts = self.model.transcript.pending_prompts();
        let footer: Option<SharedString> = if let Some(l) = look {
            l.footer.clone()
        } else if detached && self.opening() {
            Some("opening…".into())
        } else if detached {
            Some("detached: cannot answer".into())
        } else if sent {
            Some("answer sent, waiting for the daemon".into())
        } else if self.model.switching() {
            Some("switching sessions…".into())
        } else if !self.drives() {
            Some(if self.client.is_none() { "offline: nothing to answer" } else { "read-only here" }.into())
        } else if !secret && let Some(n) = more {
            Some(
                if keys {
                    format!("↓ {n} more lines: PgDn or scroll to the end")
                } else {
                    format!("↓ {n} more lines: scroll to the end to allow")
                }
                .into(),
            )
        } else if prompts.len() > 1 {
            Some(format!("+{} more waiting", prompts.len() - 1).into())
        } else {
            None
        };
        let footer_warn = look.map_or(more.is_some() && !sent, |l| l.footer_warn);
        if live {
            *self.card_look.borrow_mut() = CardLook { can, ready, keys, in_field, typed, footer: footer.clone(), footer_warn };
        }
        // The well: a guard's command alone, its `$` in `you`; else the
        // whole body, verbatim. Up to the form's most lines, the rest
        // scrolls (and ALLOW waits for its end).
        // As many whole lines as the window has room for beside the
        // composer (fit.rs: from 3, fewer if the text has fewer, to the
        // form's most), so the whole card fits the list's view.
        let form = self.layout.form();
        let shown_lines = match &c.guard {
            Some(g) => g.command.lines().count().max(1) as f32,
            None => c.body.lines().count().max(1) as f32,
        };
        let ex = self.extras();
        let floor = crate::fit::well_floor(m, form, shown_lines.clamp(1., crate::fit::WELL_MIN), ex);
        let rows = self.composer.read(cx).text().lines_len().clamp(1, self.composer_rows.max(1));
        // A soft-wrapped line takes more than its one: the most is the
        // window's room, the least the text's own lines (they scroll).
        let lines = crate::fit::well_lines(m, form, self.height, rows, floor, ex);
        let well_pad = (8. * r).round();
        let well_h = |lines: f32| lines * m.code_line + 2. * well_pad;
        let text: AnyElement = match &c.guard {
            Some(g) => h_flex()
                .items_start()
                .child(div().flex_none().text_color(p.you).child("$ "))
                .child(div().min_w_0().child(g.command.clone()))
                .into_any_element(),
            None => div().child(c.body.clone()).into_any_element(),
        };
        let has_well = !pr.prompt.is_empty();
        // Above and below the text, a border in the well's own colour, not
        // padding: GPUI clips a scroll to inside its border, so no sliver of
        // the next line shows under the last whole one.
        let well = div()
            .id("prompt-well")
            .debug_selector(|| "prompt-well".into())
            .flex_none()
            .min_h(px(well_h(floor.min(shown_lines))))
            .max_h(px(well_h(lines)))
            .overflow_y_scroll()
            .track_scroll(&c.scroll)
            .px(px((12. * r).round()))
            .map(|mut d| {
                let b = &mut d.style().border_widths;
                b.top = Some(px(well_pad).into());
                b.bottom = Some(px(well_pad).into());
                d
            })
            .border_color(p.canvas)
            .bg(p.canvas)
            .font_family(fonts::MONO)
            .text_px(m.mono)
            .line_height(px(m.code_line))
            .text_color(p.well_fg)
            .child(text);
        // Collapsing, it holds still: no breath (the beat layer's, painted
        // over the app's frame, would show through the clip).
        let g = crate::turns::live_glyph(&self.fx, glyph::Glyph::Needs, p.warn_mark, p.card, None, self.card_at.filter(|_| live), false);
        // Its frame traces from the top-left corner: one stroke along the
        // top and down the right, one down the left and along the bottom,
        // meeting after `base` (paint only, over its 1 px border).
        let trace = if live {
            self.fx.shot(self.card_at, Duration::ZERO, crate::motion::BASE, &crate::motion::ease::INOUT, crate::motion::Kind::Spatial)
        } else {
            1.
        };
        let warn_mark = p.warn_mark;
        let tracer = (trace < 1.).then(|| {
            canvas(
                |_, _, _| {},
                move |b, _, window, _| {
                    let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
                    let l = trace * (w + h);
                    let line =
                        |x: f32, y: f32, ww: f32, hh: f32| Bounds::new(point(b.origin.x + px(x), b.origin.y + px(y)), size(px(ww), px(hh)));
                    let mut q = |r: Bounds<Pixels>| window.paint_quad(fill(r, warn_mark));
                    q(line(0., 0., l.min(w), 1.));
                    q(line(w - 1., 0., 1., (l - w).clamp(0., h)));
                    q(line(0., 0., 1., l.min(h)));
                    q(line(0., h - 1., (l - h).clamp(0., w), 1.));
                },
            )
            .absolute()
            .top(px(-1.))
            .left(px(-1.))
            .right(px(-1.))
            .bottom(px(-1.))
        });
        // The dashes (F7): static quads along the top edge, inside the
        // frame, once the trace is done.
        let dashes = (trace >= 1.).then(|| {
            canvas(
                |_, _, _| {},
                move |b, _, window, _| {
                    let w = f32::from(b.size.width);
                    let mut x = 0.;
                    while x < w {
                        let run = DASH.min(w - x);
                        window.paint_quad(fill(Bounds::new(point(b.origin.x + px(x), b.origin.y), size(px(run), px(DASH_H))), warn_mark));
                        x += DASH + DASH_GAP;
                    }
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(px(DASH_H))
        });
        let head = if secret { label::SECRET } else { label::AUTHORIZE };
        let header = h_flex()
            .flex_none()
            .pt(px((12. * r).round()))
            .px(px(pad))
            .gap(px(12.))
            .items_center()
            .child(g)
            .child(display(head, fonts::size::D_M, p.warn_k).flex_none())
            .child(div().flex_1())
            .children(
                c.asked
                    .clone()
                    .filter(|_| !self.layout.tight)
                    .map(|a| mono(a, m.meta, p.dim_k).min_w_0().overflow_hidden().text_ellipsis().debug_selector(|| "card-asked".into())),
            );
        let folder = c.guard.as_ref().map(|g| {
            mono(SharedString::from(format!("in {}", g.dir)), m.meta, p.dim_k)
                .px(px(pad))
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .debug_selector(|| "card-folder".into())
        });
        let scope = pr.allow_all().filter(|_| offered).cloned();
        let check = scope.map(|s| {
            let (text, tip): (SharedString, SharedString) = (s.allow_all_label().into(), s.allow_all_scope().into());
            h_flex()
                .id("p-all")
                .debug_selector(|| "card-allow-all".into())
                .flex_none()
                .gap(px(8.))
                .items_center()
                .cursor_pointer()
                .child(
                    div()
                        .flex_none()
                        .size(px(CHECK))
                        .border_1()
                        .border_color(p.dim_k)
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(allow_all, |d| d.child(div().size(px(CHECK_IN)).bg(p.focus_k).debug_selector(|| "card-allow-all-on".into()))),
                )
                .child(div().text_px(fonts::size::TITLE).text_color(p.prose_k).whitespace_nowrap().child(text))
                .tooltip(move |window, cx| crate::overlay::tip(tip.clone(), window, cx))
                .when(live && can, |d| {
                    d.on_click(cx.listener(|this, _, _, cx| {
                        this.toggle_allow_all(cx);
                    }))
                })
        });
        let foot = footer.map(|f| {
            mono(f, m.meta, if footer_warn { p.warn_k } else { p.dim_k })
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .debug_selector(|| "card-footer".into())
        });
        let line = h_flex()
            .id("prompt-actions")
            .flex_none()
            .mt(px((12. * r).round()))
            .mb(px(pad))
            .px(px(pad))
            .h(px(crate::fit::button_h(m)))
            .gap(px(16.))
            .items_center()
            .track_scroll(&c.actions)
            .children(check)
            .children(foot)
            .child(div().flex_1())
            .children(actions);
        v_flex()
            .id("prompt-card")
            .debug_selector(move || if live { "card".into() } else { "card-exit".into() })
            .w_full()
            .border_1()
            // GPUI paints a border over the children: while the frame
            // traces (a child), the border is clear (the card's ground
            // shows through it, as it would in the card's colour).
            .border_color(if tracer.is_some() { gpui_kit::transparent_black() } else { p.warn_mark })
            .bg(p.card)
            .relative()
            .children(tracer)
            .children(dashes)
            // A click on a confirm card gives it the keys (a secret's
            // field takes them itself).
            .when(!secret && live, |d| {
                d.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        #[cfg(test)]
                        {
                            this.card_presses += 1;
                        }
                        if !this.focus.is_focused(window) {
                            window.focus(&this.focus, cx);
                            cx.notify();
                        }
                    }),
                )
            })
            .child(header)
            .when(has_well, |d| {
                // A press on the bar gives a confirm card the keys, as one on the card does.
                let bar = crate::scrollbar::Bar::new(c.scroll.clone(), &c.bar);
                let bar = if secret || !live { bar } else { bar.focus(self.focus.clone()) };
                d.child(div().mt(px((12. * r).round())).mx(px(pad)).relative().child(well).child(bar))
            })
            .children(folder.map(|f| f.mt(px((8. * r).round()))))
            .child(line)
    }
}

/// The card in its row: 8·r above it, 12·r below (polish.md §2.6), as
/// padding, so the row's measure is all of it.
fn card_slot(card: Stateful<Div>, m: &Metrics) -> Div {
    div().w_full().pt(px((8. * m.r).round())).pb(px((12. * m.r).round())).child(card)
}

/// A receipt row's height: one mono meta line with room round it.
pub(crate) fn receipt_line(m: &Metrics) -> f32 {
    m.row
}

#[cfg(test)]
mod tests {
    use super::{Answer, PromptRow, Receipt, asked, guard_shape, took, with_prompts};
    use crate::turns::{Disp, Gap, Kind, Who};
    use std::time::{Duration, Instant};

    const GUARD: &str = "Extension: guard\nTool: bash\n\nMessage from guard:\nbash wants to run a command in ~/work/relay-gateway:\n$ cargo test -p relay --test refresh_race\n\nAllow this tool call?";

    #[test]
    fn a_guard_confirm_shows_its_command_and_folder() {
        let g = guard_shape(GUARD).expect("a guard's shape");
        assert_eq!((g.command.as_ref(), g.dir.as_ref()), ("cargo test -p relay --test refresh_race", "~/work/relay-gateway"));
        assert_eq!(guard_shape(&GUARD.replace("\n", "\r\n")), Some(g), "CRLF reads the same");
        // Anything else is the whole body, as today.
        assert_eq!(guard_shape("Tool: bash\n\nInput that will run:\n$ ls\n\nAllow this tool call?"), None);
        assert_eq!(guard_shape("Extension: guard\n\nMessage from guard:\nedit writes outside the folder:\n{}"), None);
        assert_eq!(guard_shape("Activate shell_start?"), None);
    }

    #[test]
    fn the_header_says_what_asked_and_when() {
        let pr = |v: serde_json::Value| serde_json::from_value::<synaps_client::protocol::PromptRequest>(v).unwrap();
        let at = "2026-11-17T14:21:05Z";
        let local = crate::times::Stamp::local(at).unwrap().hhmm;
        let full = pr(serde_json::json!({"id": 1, "raised_at": at, "confirm_scope": {"extensions": ["guard"], "tool": "bash"}}));
        assert_eq!(asked(&full).unwrap(), format!("bash · guard · asked {local}"));
        let no_tool = pr(serde_json::json!({"id": 1, "raised_at": at, "confirm_scope": {"extensions": ["guard", "lint"]}}));
        assert_eq!(asked(&no_tool).unwrap(), format!("guard, lint · asked {local}"));
        assert_eq!(asked(&pr(serde_json::json!({"id": 1, "raised_at": at}))).unwrap(), format!("asked {local}"));
        assert_eq!(
            asked(&pr(serde_json::json!({"id": 1, "confirm_scope": {"extensions": ["guard"], "tool": "bash"}}))).unwrap(),
            "bash · guard"
        );
        assert_eq!(asked(&pr(serde_json::json!({"id": 1}))), None, "nothing known: nothing said");
    }

    #[test]
    fn a_receipt_says_what_and_when() {
        let r = |answer, took| Receipt {
            after: 1,
            own: false,
            row: 1,
            secret: false,
            answer,
            at: "14:21".into(),
            took,
            landed: Instant::now(),
        };
        let words = |r: Receipt| {
            let (w, rest) = r.words();
            format!("{w}{rest}")
        };
        assert_eq!(words(r(Answer::Allowed, Some(Duration::from_secs(7)))), "allowed 14:21 · 7s");
        assert_eq!(words(r(Answer::Denied, None)), "denied 14:21");
        assert_eq!(words(r(Answer::Elsewhere, Some(Duration::from_secs(3)))), "answered elsewhere 14:21", "not ours: no wait");
        assert_eq!(words(r(Answer::Provided, None)), "provided 14:21");
        assert_eq!((took(Duration::from_secs(125)), took(Duration::from_secs(3780))), ("2m 05s".into(), "1h 03m".into()));
    }

    #[test]
    fn a_prompt_row_follows_its_item() {
        let row = |item, id, kind| Disp { item, id, rev: 0, kind, gap: Gap::None, who: Who::None };
        let out =
            vec![row(0, 1, Kind::User), row(1, 2, Kind::Summary { end: 4, open: false }), row(4, 5, Kind::Work), row(5, 6, Kind::Prose)];
        let p = |after| PromptRow { after, id: 99 + after as u64, rev: 0 };
        let ids = |v: Vec<Disp>| v.iter().map(|d| d.id).collect::<Vec<_>>();
        assert_eq!(ids(with_prompts(out.clone(), &[p(4)], 0)), [1, 2, 5, 103, 6], "after its call");
        assert_eq!(ids(with_prompts(out.clone(), &[p(2)], 0)), [1, 2, 101, 5, 6], "in a folded run: after the run");
        assert_eq!(ids(with_prompts(out.clone(), &[p(5)], 0)), [1, 2, 5, 6, 104], "at the end");
        assert_eq!(ids(with_prompts(out[2..].to_vec(), &[p(2), p(5)], 4)), [5, 6, 104], "a rebuild from 4 adds only its own");
    }
}
